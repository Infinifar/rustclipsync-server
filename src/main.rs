//! RustClipSync's latest-value V1 server. Legacy SyncClipboard endpoints are
//! deliberately not exposed; see PROTOCOL-V1.md for the wire contract.
use std::{
    env,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use axum::{
    Router,
    http::{HeaderMap, StatusCode, header},
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::Connection;
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Semaphore, broadcast};

mod database;
mod v1;

const MAX_TEXT: usize = 10 * 1024 * 1024;

struct App {
    db: Mutex<Connection>,
    user: String,
    password: String,
    v1_events: broadcast::Sender<()>,
    v1_waiters: Arc<Semaphore>,
    v1_uploads: Arc<Semaphore>,
}
type Shared = Arc<App>;

fn require(headers: &HeaderMap, app: &App) -> Result<(), StatusCode> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Basic ")
                .or_else(|| v.strip_prefix("basic "))
        })
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let clear = std::str::from_utf8(&decoded).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let (user, password) = clear.split_once(':').ok_or(StatusCode::UNAUTHORIZED)?;
    let good = user.as_bytes().ct_eq(app.user.as_bytes())
        & password.as_bytes().ct_eq(app.password.as_bytes());
    if bool::from(good) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn db_error(_: rusqlite::Error) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}
async fn health() -> &'static str {
    "ok"
}

fn router(app: Shared) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .merge(v1::router())
        .with_state(app)
}

/// Old records are not exposed through V1, but still obey the configured
/// retention until the operator deliberately removes them.
fn clean_legacy_history(db: &Connection, days: u32) -> rusqlite::Result<usize> {
    db.execute(
        "DELETE FROM history WHERE unixepoch(json_extract(profile, '$.syncedAt')) < unixepoch() - ?1
         AND hash NOT IN (SELECT json_extract(profile, '$.hash') FROM current WHERE id = 1)",
        [i64::from(days) * 86_400],
    )
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let command = env::args().nth(1);
    if command.as_deref() == Some("--healthcheck") {
        let listen: SocketAddr = env::var("RCS_LISTEN")
            .unwrap_or_else(|_| "127.0.0.1:8090".into())
            .parse()?;
        let target = SocketAddr::new("127.0.0.1".parse()?, listen.port());
        let mut socket = TcpStream::connect_timeout(&target, Duration::from_secs(2))?;
        socket.set_read_timeout(Some(Duration::from_secs(2)))?;
        socket
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        let mut response = [0; 32];
        let n = socket.read(&mut response)?;
        if !response[..n].starts_with(b"HTTP/1.1 200") {
            return Err("health check failed".into());
        }
        return Ok(());
    }
    let path = PathBuf::from(env::var("RCS_DATA_DIR").unwrap_or_else(|_| "./data".into()))
        .join("clipsync.db");
    let db_key = database::read_key()?;
    if command.as_deref() == Some("--migrate-db") {
        database::migrate(&path, &db_key)?;
        return Ok(());
    }
    if command.as_deref() == Some("--rotate-v1-epoch") {
        let mut db = database::open(&path, &db_key)?;
        v1::rotate_epoch(&mut db)?;
        return Ok(());
    }
    if command.is_some() {
        return Err("unknown command".into());
    }
    let user = env::var("RCS_USER").map_err(|_| "RCS_USER is required")?;
    let password = if let Ok(path) = env::var("RCS_PASSWORD_FILE") {
        std::fs::read_to_string(path)?
            .trim_end_matches(['\r', '\n'])
            .to_owned()
    } else {
        env::var("RCS_PASSWORD").map_err(|_| "RCS_PASSWORD or RCS_PASSWORD_FILE is required")?
    };
    if user.is_empty()
        || user.len() > 512
        || user.contains([':', '\r', '\n'])
        || !(12..=4096).contains(&password.len())
    {
        return Err("user must be valid and password must be 12 to 4096 bytes".into());
    }
    let retention_days: u32 = env::var("RCS_HISTORY_RETENTION_DAYS")
        .unwrap_or_else(|_| "7".into())
        .parse()
        .map_err(|_| "RCS_HISTORY_RETENTION_DAYS must be an integer from 1 to 3650")?;
    if !(1..=3650).contains(&retention_days) {
        return Err("RCS_HISTORY_RETENTION_DAYS must be an integer from 1 to 3650".into());
    }
    let listen: SocketAddr = env::var("RCS_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8090".into())
        .parse()?;
    let (v1_events, _) = broadcast::channel(64);
    let app = Arc::new(App {
        db: Mutex::new(database::open(&path, &db_key)?),
        user,
        password,
        v1_events,
        v1_waiters: Arc::new(Semaphore::new(128)),
        v1_uploads: Arc::new(Semaphore::new(8)),
    });
    let cleanup_app = Arc::clone(&app);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let db = cleanup_app.db.lock().await;
            if clean_legacy_history(&db, retention_days).is_err() {
                eprintln!("old history cleanup failed");
            }
            if v1::clean_history(&db, retention_days).is_err() {
                eprintln!("history cleanup failed");
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, router(app)).await?;
    Ok(())
}
