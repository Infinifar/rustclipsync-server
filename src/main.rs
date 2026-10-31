use std::{
    collections::HashMap,
    env,
    io::{Read, Write},
    net::SocketAddr,
    net::TcpStream,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;

mod database;

const MAX_TEXT: usize = 10 * 1024 * 1024;
const MAX_META: usize = 256 * 1024;
const MAX_WS: usize = 16 * 1024;
const RS: char = '\u{1e}';

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, deserialize_with = "deserialize_optional_hash")]
    hash: String,
    text: String,
    has_data: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer_data_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    synced_at: Option<DateTime<Utc>>,
}

impl Profile {
    fn empty() -> Self {
        Self {
            kind: "Text".into(),
            hash: digest(b""),
            text: String::new(),
            has_data: false,
            data_name: None,
            transfer_data_hash: None,
            size: Some(0),
            synced_at: None,
        }
    }
}

struct App {
    db: Mutex<Connection>,
    user: String,
    password: String,
    dedupe_window_seconds: u32,
    events: broadcast::Sender<Profile>,
    tokens: Mutex<HashMap<String, Instant>>,
}
type Shared = Arc<App>;
type ApiResult = Result<Response, StatusCode>;

fn deserialize_optional_hash<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn digest(data: &[u8]) -> String {
    format!("{:X}", Sha256::digest(data))
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit())
}
fn valid_name(name: &str) -> bool {
    name.len() <= 128
        && name.ends_with(".txt")
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        && !name.starts_with('.')
        && !name.contains("..")
}
fn authorized(headers: &HeaderMap, app: &App) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(encoded) = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))
    else {
        return false;
    };
    let Ok(bytes) = STANDARD.decode(encoded) else {
        return false;
    };
    let Ok(clear) = std::str::from_utf8(&bytes) else {
        return false;
    };
    let Some((user, pass)) = clear.split_once(':') else {
        return false;
    };
    user.as_bytes().ct_eq(app.user.as_bytes()).into()
        && pass.as_bytes().ct_eq(app.password.as_bytes()).into()
}
fn require(headers: &HeaderMap, app: &App) -> Result<(), StatusCode> {
    if authorized(headers, app) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
fn db_error(_: rusqlite::Error) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}
fn profile_response(profile: Profile) -> Response {
    Json(profile).into_response()
}

async fn root(State(app): State<Shared>, headers: HeaderMap) -> ApiResult {
    require(&headers, &app)?;
    Ok("Server is running.".into_response())
}
async fn health() -> &'static str {
    "ok"
}
async fn current(State(app): State<Shared>, headers: HeaderMap) -> ApiResult {
    require(&headers, &app)?;
    let db = app.db.lock().await;
    let data: Option<String> = db
        .query_row("SELECT profile FROM current WHERE id=1", [], |r| r.get(0))
        .optional()
        .map_err(db_error)?;
    let profile = data
        .map(|s| serde_json::from_str(&s).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR))
        .transpose()?
        .unwrap_or_else(Profile::empty);
    Ok(profile_response(profile))
}
async fn upload(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Bytes,
) -> ApiResult {
    require(&headers, &app)?;
    if !valid_name(&name) || body.len() > MAX_TEXT {
        return Err(StatusCode::BAD_REQUEST);
    }
    std::str::from_utf8(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let hash = digest(&body);
    let db = app.db.lock().await;
    db.execute("DELETE FROM pending WHERE created < unixepoch()-3600", [])
        .map_err(db_error)?;
    let count: i64 = db
        .query_row("SELECT count(*) FROM pending", [], |r| r.get(0))
        .map_err(db_error)?;
    if count >= 16
        && db
            .query_row("SELECT 1 FROM pending WHERE name=?1", [&name], |_| Ok(()))
            .optional()
            .map_err(db_error)?
            .is_none()
    {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    db.execute("INSERT INTO pending(name,hash,data,created) VALUES(?1,?2,?3,unixepoch()) ON CONFLICT(name) DO UPDATE SET hash=excluded.hash,data=excluded.data,created=excluded.created", params![name,hash,body.as_ref()]).map_err(db_error)?;
    Ok(StatusCode::OK.into_response())
}
fn normalize(mut p: Profile, full: &[u8]) -> Result<Profile, StatusCode> {
    if p.kind != "Text" || p.text.len() > MAX_TEXT {
        return Err(StatusCode::BAD_REQUEST);
    }
    let content = std::str::from_utf8(full).map_err(|_| StatusCode::BAD_REQUEST)?;
    let hash = digest(full);
    if !p.hash.is_empty() && (!valid_hash(&p.hash) || !p.hash.eq_ignore_ascii_case(&hash)) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if p.has_data {
        if !content.starts_with(&p.text) {
            return Err(StatusCode::BAD_REQUEST);
        }
        if p.transfer_data_hash
            .as_ref()
            .is_some_and(|h| !h.eq_ignore_ascii_case(&hash))
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        p.transfer_data_hash = Some(hash.clone());
    } else if p.data_name.is_some() || p.transfer_data_hash.is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if p.size
        .is_some_and(|size| size != content.encode_utf16().count())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    p.hash = hash;
    p.size = Some(content.encode_utf16().count());
    p.synced_at = Some(Utc::now());
    Ok(p)
}
async fn commit(State(app): State<Shared>, headers: HeaderMap, body: Bytes) -> ApiResult {
    require(&headers, &app)?;
    if body.len() > MAX_META {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let incoming: Profile = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if incoming.kind != "Text" {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    if incoming.has_data && !incoming.data_name.as_deref().is_some_and(valid_name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut db = app.db.lock().await;
    let tx = db.transaction().map_err(db_error)?;
    let full: Vec<u8> = if incoming.has_data {
        let name = incoming.data_name.as_ref().unwrap();
        let pending: Option<Vec<u8>> = tx
            .query_row("SELECT data FROM pending WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .optional()
            .map_err(db_error)?;
        if let Some(bytes) = pending {
            bytes
        } else if valid_hash(&incoming.hash) {
            tx.query_row(
                "SELECT data FROM history WHERE hash=?1",
                [incoming.hash.to_ascii_uppercase()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?
            .ok_or(StatusCode::NOT_FOUND)?
        } else {
            return Err(StatusCode::NOT_FOUND);
        }
    } else {
        incoming.text.as_bytes().to_vec()
    };
    if full.len() > MAX_TEXT {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let profile = normalize(incoming, &full)?;
    let previous: Option<String> = tx
        .query_row("SELECT profile FROM current WHERE id=1", [], |r| r.get(0))
        .optional()
        .map_err(db_error)?;
    if let Some(previous) = previous {
        let previous: Profile =
            serde_json::from_str(&previous).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let duplicate = previous.hash == profile.hash
            && previous
                .synced_at
                .zip(profile.synced_at)
                .is_some_and(|(old, now)| {
                    let elapsed = now.signed_duration_since(old).num_milliseconds();
                    elapsed >= 0 && elapsed < i64::from(app.dedupe_window_seconds) * 1000
                });
        if duplicate {
            if let Some(name) = &profile.data_name {
                tx.execute("DELETE FROM pending WHERE name=?1", [name])
                    .map_err(db_error)?;
            }
            tx.commit().map_err(db_error)?;
            return Ok(StatusCode::OK.into_response());
        }
    }
    let serialized =
        serde_json::to_string(&profile).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    tx.execute("INSERT INTO history(hash,profile,data) VALUES(?1,?2,?3) ON CONFLICT(hash) DO UPDATE SET profile=excluded.profile", params![profile.hash, serialized, full]).map_err(db_error)?;
    tx.execute("INSERT INTO current(id,profile) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET profile=excluded.profile", [&serialized]).map_err(db_error)?;
    if let Some(name) = &profile.data_name {
        tx.execute("DELETE FROM pending WHERE name=?1", [name])
            .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;
    drop(db);
    let _ = app.events.send(profile);
    Ok(StatusCode::OK.into_response())
}
async fn history(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> ApiResult {
    require(&headers, &app)?;
    let Some(hash) = hash.strip_prefix("Text-") else {
        return Err(StatusCode::NOT_FOUND);
    };
    if !valid_hash(hash) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = app.db.lock().await;
    let data: Option<String> = db
        .query_row(
            "SELECT profile FROM history WHERE hash=?1",
            [hash.to_ascii_uppercase()],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let Some(data) = data else {
        return Err(StatusCode::NOT_FOUND);
    };
    let p: Profile = serde_json::from_str(&data).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(
        Json(json!({"hash":p.hash,"text":p.text,"type":"Text","size":p.size,"hasData":p.has_data}))
            .into_response(),
    )
}
async fn data_by_hash(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> ApiResult {
    require(&headers, &app)?;
    let Some(hash) = hash.strip_prefix("Text-") else {
        return Err(StatusCode::NOT_FOUND);
    };
    if !valid_hash(hash) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = app.db.lock().await;
    let data: Option<Vec<u8>> = db
        .query_row(
            "SELECT data FROM history WHERE hash=?1",
            [hash.to_ascii_uppercase()],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    Ok(data.ok_or(StatusCode::NOT_FOUND)?.into_response())
}
async fn data_by_name(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult {
    require(&headers, &app)?;
    if !valid_name(&name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = app.db.lock().await;
    let data: Option<Vec<u8>> = db.query_row("SELECT h.data FROM history h JOIN current c ON json_extract(c.profile,'$.hash')=h.hash WHERE json_extract(c.profile,'$.dataName')=?1 LIMIT 1", [name], |r| r.get(0)).optional().map_err(db_error)?;
    Ok(data.ok_or(StatusCode::NOT_FOUND)?.into_response())
}
#[derive(Deserialize)]
struct NegotiateQuery {
    #[serde(rename = "negotiateVersion")]
    negotiate_version: Option<u8>,
}
async fn negotiate(
    State(app): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<NegotiateQuery>,
) -> ApiResult {
    require(&headers, &app)?;
    if q.negotiate_version != Some(1) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let token = Uuid::new_v4().to_string();
    let mut tokens = app.tokens.lock().await;
    tokens.retain(|_, t| t.elapsed() < Duration::from_secs(30));
    if tokens.len() >= 100 {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    tokens.insert(token.clone(), Instant::now());
    Ok(Json(json!({"negotiateVersion":1,"connectionId":token,"connectionToken":token,"availableTransports":[{"transport":"WebSockets","transferFormats":["Text"]}]})).into_response())
}
#[derive(Deserialize)]
struct WsQuery {
    id: String,
}
async fn ws(
    State(app): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<WsQuery>,
    upgrade: WebSocketUpgrade,
) -> ApiResult {
    require(&headers, &app)?;
    let valid = app
        .tokens
        .lock()
        .await
        .remove(&q.id)
        .is_some_and(|t| t.elapsed() < Duration::from_secs(30));
    if !valid {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(upgrade
        .max_message_size(MAX_WS)
        .on_upgrade(move |socket| serve_ws(app, socket))
        .into_response())
}
async fn serve_ws(app: Shared, mut socket: WebSocket) {
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next()).await;
    let Some(Ok(Message::Text(text))) = first.ok().flatten() else {
        return;
    };
    if text.len() > MAX_WS
        || !text.ends_with(RS)
        || serde_json::from_str::<Value>(text.trim_end_matches(RS))
            .ok()
            .is_none_or(|v| {
                v.get("protocol").and_then(Value::as_str) != Some("json")
                    || v.get("version").and_then(Value::as_u64) != Some(1)
            })
    {
        return;
    }
    if socket
        .send(Message::Text(format!("{{}}{RS}").into()))
        .await
        .is_err()
    {
        return;
    }
    let mut events = app.events.subscribe();
    let mut pulse = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            msg = socket.next() => match msg {
                Some(Ok(Message::Text(text))) if text.len() <= MAX_WS => {
                    if !text.split(RS).filter(|s| !s.is_empty()).all(|s| serde_json::from_str::<Value>(s).is_ok()) { break }
                },
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {},
                _ => break,
            },
            event = events.recv() => match event {
                Ok(profile) => {
                    let payload = json!({"type":1,"target":"RemoteProfileChanged","arguments":[profile]});
                    if socket.send(Message::Text(format!("{payload}{RS}").into())).await.is_err() { break }
                },
                Err(broadcast::error::RecvError::Lagged(_)) => break,
                Err(_) => break,
            },
            _ = pulse.tick() => if socket.send(Message::Text(format!("{{\"type\":6}}{RS}").into())).await.is_err() { break },
        }
    }
}

fn router(app: Shared) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/", get(root))
        .route(
            "/SyncClipboard.json",
            get(current)
                .put(commit)
                .layer(DefaultBodyLimit::max(MAX_META)),
        )
        .route(
            "/file/{name}",
            get(data_by_name)
                .head(data_by_name)
                .put(upload)
                .layer(DefaultBodyLimit::max(MAX_TEXT)),
        )
        .route("/api/history/{hash}", get(history))
        .route("/api/history/{hash}/data", get(data_by_hash))
        .route("/SyncClipboardHub/negotiate", post(negotiate))
        .route("/SyncClipboardHub", get(ws))
        .with_state(app)
}

fn clean_history(db: &Connection, days: u32) -> rusqlite::Result<usize> {
    // Keep the current clipboard even if its history entry is older than the limit.
    db.execute(
        "DELETE FROM history WHERE unixepoch(json_extract(profile, '$.syncedAt')) < unixepoch() - ?1 AND hash NOT IN (SELECT json_extract(profile, '$.hash') FROM current WHERE id = 1)",
        [i64::from(days) * 86_400],
    )
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if env::args().nth(1).as_deref() == Some("--healthcheck") {
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
    if env::args().nth(1).as_deref() == Some("--migrate-db") {
        database::migrate(&path, &db_key)?;
        return Ok(());
    }
    let user = env::var("RCS_USER").map_err(|_| "RCS_USER is required")?;
    let password = if let Ok(path) = env::var("RCS_PASSWORD_FILE") {
        std::fs::read_to_string(path)?
            .trim_end_matches(['\r', '\n'])
            .to_owned()
    } else {
        env::var("RCS_PASSWORD").map_err(|_| "RCS_PASSWORD or RCS_PASSWORD_FILE is required")?
    };
    if user.is_empty() || password.len() < 12 {
        return Err("user must be nonempty and password at least 12 characters".into());
    }
    let retention_days: u32 = env::var("RCS_HISTORY_RETENTION_DAYS")
        .unwrap_or_else(|_| "7".into())
        .parse()
        .map_err(|_| "RCS_HISTORY_RETENTION_DAYS must be an integer from 1 to 3650")?;
    if !(1..=3650).contains(&retention_days) {
        return Err("RCS_HISTORY_RETENTION_DAYS must be an integer from 1 to 3650".into());
    }
    let dedupe_window_seconds: u32 = env::var("RCS_DEDUP_WINDOW_SECONDS")
        .unwrap_or_else(|_| "60".into())
        .parse()
        .map_err(|_| "RCS_DEDUP_WINDOW_SECONDS must be an integer from 0 to 86400")?;
    if dedupe_window_seconds > 86400 {
        return Err("RCS_DEDUP_WINDOW_SECONDS must be an integer from 0 to 86400".into());
    }
    let listen: SocketAddr = env::var("RCS_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8090".into())
        .parse()?;
    let (events, _) = broadcast::channel(64);
    let app = Arc::new(App {
        db: Mutex::new(database::open(&path, &db_key)?),
        user,
        password,
        dedupe_window_seconds,
        events,
        tokens: Mutex::new(HashMap::new()),
    });
    let cleanup_app = Arc::clone(&app);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let db = cleanup_app.db.lock().await;
            if clean_history(&db, retention_days).is_err() {
                eprintln!("history cleanup failed");
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, router(app)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request},
    };
    use futures_util::SinkExt;
    use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};
    use tower::ServiceExt;

    #[test]
    fn cleanup_keeps_current_and_unknown_dates() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE current(id INTEGER PRIMARY KEY, profile TEXT NOT NULL); CREATE TABLE history(hash TEXT PRIMARY KEY, profile TEXT NOT NULL, data BLOB NOT NULL);").unwrap();
        for (hash, date) in [
            ("old", "2000-01-01T00:00:00Z"),
            ("current", "2000-01-01T00:00:00Z"),
            ("unknown", "invalid"),
            ("recent", "2999-01-01T00:00:00Z"),
        ] {
            db.execute(
                "INSERT INTO history VALUES (?1, ?2, X'')",
                params![hash, json!({"hash":hash,"syncedAt":date}).to_string()],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO current VALUES (1, ?1)",
            [json!({"hash":"current"}).to_string()],
        )
        .unwrap();
        assert_eq!(clean_history(&db, 7).unwrap(), 1);
        let hashes: Vec<String> = db
            .prepare("SELECT hash FROM history ORDER BY hash")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(hashes, ["current", "recent", "unknown"]);
    }

    fn test_state() -> Shared {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE current(id INTEGER PRIMARY KEY, profile TEXT NOT NULL);
            CREATE TABLE history(hash TEXT PRIMARY KEY, profile TEXT NOT NULL, data BLOB NOT NULL);
            CREATE TABLE pending(name TEXT PRIMARY KEY, hash TEXT NOT NULL, data BLOB NOT NULL, created INTEGER NOT NULL);").unwrap();
        let (events, _) = broadcast::channel(16);
        Arc::new(App {
            db: Mutex::new(db),
            user: "test".into(),
            password: "long-password".into(),
            dedupe_window_seconds: 60,
            events,
            tokens: Mutex::new(HashMap::new()),
        })
    }
    fn test_app() -> Router {
        router(test_state())
    }
    async fn call(app: &Router, method: Method, path: &str, body: Vec<u8>, auth: bool) -> Response {
        let mut request = Request::builder().method(method).uri(path);
        if auth {
            request = request.header(
                header::AUTHORIZATION,
                format!("Basic {}", STANDARD.encode("test:long-password")),
            );
        }
        app.clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn accepts_phone_text_without_hash_and_validates_supplied_hash() {
        let app = test_app();
        for payload in [
            json!({"hasData":false,"text":"手机测试🙂","type":"Text"}),
            json!({"hasData":false,"text":"手机测试🙂","type":"Text","hash":null}),
        ] {
            let response = call(
                &app,
                Method::PUT,
                "/SyncClipboard.json",
                payload.to_string().into_bytes(),
                true,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = call(&app, Method::GET, "/SyncClipboard.json", vec![], true).await;
        let profile: Profile =
            serde_json::from_slice(&to_bytes(response.into_body(), MAX_META).await.unwrap())
                .unwrap();
        assert_eq!(profile.hash, digest("手机测试🙂".as_bytes()));
        let invalid =
            json!({"hasData":false,"text":"手机测试🙂","type":"Text","hash":"0".repeat(64)});
        assert_eq!(
            call(
                &app,
                Method::PUT,
                "/SyncClipboard.json",
                invalid.to_string().into_bytes(),
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    #[tokio::test]
    async fn duplicate_current_is_suppressed_but_changes_and_expired_repeat_are_broadcast() {
        let state = test_state();
        let mut events = state.events.subscribe();
        let app = router(state.clone());
        let profile = |text: &str| {
            json!({"type":"Text","hash":digest(text.as_bytes()),"text":text,"hasData":false})
                .to_string()
                .into_bytes()
        };
        assert_eq!(
            call(&app, Method::PUT, "/SyncClipboard.json", profile("A"), true)
                .await
                .status(),
            StatusCode::OK
        );
        assert!(events.try_recv().is_ok());
        assert_eq!(
            call(&app, Method::PUT, "/SyncClipboard.json", profile("A"), true)
                .await
                .status(),
            StatusCode::OK
        );
        assert!(events.try_recv().is_err());
        for text in ["B", "A"] {
            assert_eq!(
                call(
                    &app,
                    Method::PUT,
                    "/SyncClipboard.json",
                    profile(text),
                    true
                )
                .await
                .status(),
                StatusCode::OK
            );
            assert!(events.try_recv().is_ok());
        }
        state
            .db
            .lock()
            .await
            .execute(
                "UPDATE current SET profile=json_set(profile,'$.syncedAt','2000-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        assert_eq!(
            call(&app, Method::PUT, "/SyncClipboard.json", profile("A"), true)
                .await
                .status(),
            StatusCode::OK
        );
        assert!(events.try_recv().is_ok());
    }

    #[tokio::test]
    async fn duplicate_long_text_removes_pending_without_event() {
        let state = test_state();
        let mut events = state.events.subscribe();
        let app = router(state.clone());
        let text = "test long text";
        let name = "duplicate.txt";
        let body = json!({"type":"Text","hash":digest(text.as_bytes()),"text":"test","hasData":true,"dataName":name}).to_string().into_bytes();
        for attempt in 0..2 {
            assert_eq!(
                call(
                    &app,
                    Method::PUT,
                    "/file/duplicate.txt",
                    text.as_bytes().to_vec(),
                    true
                )
                .await
                .status(),
                StatusCode::OK
            );
            assert_eq!(
                call(&app, Method::PUT, "/SyncClipboard.json", body.clone(), true)
                    .await
                    .status(),
                StatusCode::OK
            );
            assert_eq!(events.try_recv().is_ok(), attempt == 0);
            let count: i64 = state
                .db
                .lock()
                .await
                .query_row("SELECT count(*) FROM pending", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[tokio::test]
    async fn concurrent_duplicate_is_atomic_and_zero_disables_filter() {
        let state = test_state();
        let mut events = state.events.subscribe();
        let app = router(state.clone());
        let body = json!({"type":"Text","hash":digest(b"same"),"text":"same","hasData":false})
            .to_string()
            .into_bytes();
        let (first, second) = tokio::join!(
            call(&app, Method::PUT, "/SyncClipboard.json", body.clone(), true),
            call(&app, Method::PUT, "/SyncClipboard.json", body.clone(), true),
        );
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        assert!(events.try_recv().is_ok());
        assert!(events.try_recv().is_err());

        let mut unshared = Arc::try_unwrap(test_state()).ok().unwrap();
        unshared.dedupe_window_seconds = 0;
        let state = Arc::new(unshared);
        let mut events = state.events.subscribe();
        let app = router(state);
        for _ in 0..2 {
            assert_eq!(
                call(&app, Method::PUT, "/SyncClipboard.json", body.clone(), true)
                    .await
                    .status(),
                StatusCode::OK
            );
            assert!(events.try_recv().is_ok());
        }
    }
    #[tokio::test]
    async fn short_text_auth_and_hash() {
        let app = test_app();
        assert_eq!(
            call(&app, Method::GET, "/SyncClipboard.json", vec![], false)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let empty = call(&app, Method::GET, "/SyncClipboard.json", vec![], true).await;
        assert_eq!(empty.status(), StatusCode::OK);
        let initial: Value =
            serde_json::from_slice(&to_bytes(empty.into_body(), MAX_META).await.unwrap()).unwrap();
        assert_eq!(initial["hash"], digest(b""));
        let text = "你好\n🙂";
        let hash = digest(text.as_bytes());
        let profile = json!({"type":"Text","hash":hash,"text":text,"hasData":false,"size":text.encode_utf16().count()});
        assert_eq!(
            call(
                &app,
                Method::PUT,
                "/SyncClipboard.json",
                profile.to_string().into_bytes(),
                true
            )
            .await
            .status(),
            StatusCode::OK
        );
        let bad = json!({"type":"Text","hash":"0".repeat(64),"text":"corrupt","hasData":false});
        assert_eq!(
            call(
                &app,
                Method::PUT,
                "/SyncClipboard.json",
                bad.to_string().into_bytes(),
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        let current = call(&app, Method::GET, "/SyncClipboard.json", vec![], true).await;
        let value: Value =
            serde_json::from_slice(&to_bytes(current.into_body(), MAX_META).await.unwrap())
                .unwrap();
        assert_eq!(value["hash"], hash);
        assert!(value["syncedAt"].is_string());
    }
    #[tokio::test]
    async fn long_text_round_trip_and_rejection() {
        let app = test_app();
        let text = "数据🙂".repeat(12000);
        let hash = digest(text.as_bytes());
        let name = format!("Text_{hash}.txt");
        assert_eq!(
            call(
                &app,
                Method::PUT,
                "/file/../escape.txt",
                text.as_bytes().to_vec(),
                true
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(
                &app,
                Method::PUT,
                &format!("/file/{name}"),
                text.as_bytes().to_vec(),
                true
            )
            .await
            .status(),
            StatusCode::OK
        );
        let p = json!({"type":"Text","hash":hash,"text":"数据🙂","hasData":true,"dataName":name,"transferDataHash":hash,"size":text.encode_utf16().count()});
        assert_eq!(
            call(
                &app,
                Method::PUT,
                "/SyncClipboard.json",
                p.to_string().into_bytes(),
                true
            )
            .await
            .status(),
            StatusCode::OK
        );
        let found = call(
            &app,
            Method::GET,
            &format!("/api/history/Text-{hash}"),
            vec![],
            true,
        )
        .await;
        assert_eq!(found.status(), StatusCode::OK);
        let data = call(
            &app,
            Method::GET,
            &format!("/api/history/Text-{hash}/data"),
            vec![],
            true,
        )
        .await;
        assert_eq!(
            &to_bytes(data.into_body(), MAX_TEXT).await.unwrap()[..],
            text.as_bytes()
        );
        let file = call(&app, Method::GET, &format!("/file/{name}"), vec![], true).await;
        assert_eq!(
            &to_bytes(file.into_body(), MAX_TEXT).await.unwrap()[..],
            text.as_bytes()
        );
        let unsupported = json!({"type":"Image","hash":hash,"text":"","hasData":false});
        assert_eq!(
            call(
                &app,
                Method::PUT,
                "/SyncClipboard.json",
                unsupported.to_string().into_bytes(),
                true
            )
            .await
            .status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
    #[tokio::test]
    async fn negotiate_is_signalr_shaped() {
        let app = test_app();
        let response = call(
            &app,
            Method::POST,
            "/SyncClipboardHub/negotiate?negotiateVersion=1",
            vec![],
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), MAX_META).await.unwrap())
                .unwrap();
        assert!(value["connectionToken"].as_str().is_some());
        assert_eq!(value["availableTransports"][0]["transport"], "WebSockets");
    }

    #[tokio::test]
    async fn websocket_delivers_signalr_change() {
        let app = test_state();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_app = app.clone();
        let server = tokio::spawn(async move { axum::serve(listener, router(server_app)).await });
        let client = reqwest::Client::new();
        let negotiation: Value = client
            .post(format!(
                "http://{address}/SyncClipboardHub/negotiate?negotiateVersion=1"
            ))
            .basic_auth("test", Some("long-password"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let token = negotiation["connectionToken"].as_str().unwrap();
        let mut request = format!("ws://{address}/SyncClipboardHub?id={token}")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Basic {}", STANDARD.encode("test:long-password"))
                .parse()
                .unwrap(),
        );
        let (mut socket, _) = connect_async(request).await.unwrap();
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                format!("{{\"protocol\":\"json\",\"version\":1}}{RS}").into(),
            ))
            .await
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reply.to_text().unwrap(), format!("{{}}{RS}"));
        let profile = Profile::empty();
        app.events.send(profile).unwrap();
        loop {
            let reply = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let value: Value =
                serde_json::from_str(reply.to_text().unwrap().trim_end_matches(RS)).unwrap();
            if value["type"] == 1 {
                assert_eq!(value["target"], "RemoteProfileChanged");
                assert_eq!(value["arguments"][0]["type"], "Text");
                break;
            }
        }
        server.abort();
    }

    #[tokio::test]
    async fn committed_text_survives_reopen_and_failed_commit_does_not_replace_it() {
        let directory = std::env::temp_dir().join(format!("rustclipsync-test-{}", Uuid::new_v4()));
        let path = directory.join("clipsync.db");
        let key = "ab".repeat(32);
        let db = database::open(&path, &key).unwrap();
        let (events, _) = broadcast::channel(16);
        let app = router(Arc::new(App {
            db: Mutex::new(db),
            user: "test".into(),
            password: "long-password".into(),
            dedupe_window_seconds: 60,
            events,
            tokens: Mutex::new(HashMap::new()),
        }));
        let text = "persisted\n文本";
        let hash = digest(text.as_bytes());
        let valid = json!({"type":"Text","hash":hash,"text":text,"hasData":false});
        assert_eq!(
            call(
                &app,
                Method::PUT,
                "/SyncClipboard.json",
                valid.to_string().into_bytes(),
                true
            )
            .await
            .status(),
            StatusCode::OK
        );
        let missing = json!({"type":"Text","hash":"A".repeat(64),"text":"preview","hasData":true,"dataName":"missing.txt"});
        assert_eq!(
            call(
                &app,
                Method::PUT,
                "/SyncClipboard.json",
                missing.to_string().into_bytes(),
                true
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        drop(app);
        let reopened = database::open(&path, &key).unwrap();
        let profile: String = reopened
            .query_row("SELECT profile FROM current WHERE id=1", [], |r| r.get(0))
            .unwrap();
        let profile: Profile = serde_json::from_str(&profile).unwrap();
        assert_eq!(profile.hash, hash);
        let content: Vec<u8> = reopened
            .query_row("SELECT data FROM history WHERE hash=?1", [hash], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(content, text.as_bytes());
        drop(reopened);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
