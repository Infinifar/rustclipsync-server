//! Versioned, latest-value clipboard protocol. All payloads are stored in the
//! same SQLCipher database as the existing service; notifications carry no text.

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::OwnedSemaphorePermit;
use uuid::Uuid;

use crate::{MAX_TEXT, Shared, db_error, require};

const MAX_DEVICES: i64 = 32;
const RECEIPT_DAYS: u32 = 30;

pub fn init(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS v1_meta(
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
            epoch TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision >= 0));
         CREATE TABLE IF NOT EXISTS v1_devices(
            id TEXT PRIMARY KEY, token_hash BLOB NOT NULL,
            ack_revision INTEGER NOT NULL CHECK(ack_revision >= 0));
         CREATE TABLE IF NOT EXISTS v1_messages(
            revision INTEGER PRIMARY KEY, message_id TEXT NOT NULL UNIQUE,
            device_id TEXT NOT NULL, base_revision INTEGER NOT NULL,
            body_hash BLOB NOT NULL, body BLOB, created INTEGER NOT NULL,
            sensitive INTEGER NOT NULL DEFAULT 0 CHECK(sensitive IN (0,1)));
         CREATE TABLE IF NOT EXISTS v1_cancelled(
            message_id TEXT PRIMARY KEY, device_id TEXT NOT NULL, created INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS v1_messages_created ON v1_messages(created);",
    )?;
    let has_sensitive: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('v1_messages') WHERE name='sensitive')",
        [],
        |r| r.get(0),
    )?;
    if !has_sensitive {
        db.execute("ALTER TABLE v1_messages ADD COLUMN sensitive INTEGER NOT NULL DEFAULT 0 CHECK(sensitive IN (0,1))", [])?;
    }
    db.execute(
        "INSERT OR IGNORE INTO v1_meta(singleton, epoch, revision) VALUES(1, ?1, 0)",
        [Uuid::new_v4().to_string()],
    )?;
    Ok(())
}

/// Run only while the service is stopped, after restoring an older backup.
/// Invalidate every device so none can use a cursor from the previous epoch.
pub fn rotate_epoch(db: &mut Connection) -> rusqlite::Result<()> {
    let tx = db.transaction()?;
    tx.execute("DELETE FROM v1_devices", [])?;
    tx.execute(
        "UPDATE v1_meta SET epoch=?1 WHERE singleton=1",
        [Uuid::new_v4().to_string()],
    )?;
    tx.commit()
}

/// Keep the current value even when old. Retain idempotency receipts for at
/// least 30 days, independently of the configured text retention period.
pub fn clean_history(db: &Connection, days: u32) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE v1_messages SET body = NULL WHERE body IS NOT NULL
         AND created < unixepoch() - ?1
         AND revision != (SELECT revision FROM v1_meta WHERE singleton = 1)",
        [i64::from(days) * 86_400],
    )?;
    db.execute(
        "DELETE FROM v1_messages WHERE created < unixepoch() - ?1
         AND revision != (SELECT revision FROM v1_meta WHERE singleton = 1)",
        [i64::from(days.max(RECEIPT_DAYS)) * 86_400],
    )?;
    db.execute(
        "DELETE FROM v1_cancelled WHERE created < unixepoch() - ?1",
        [i64::from(RECEIPT_DAYS) * 86_400],
    )?;
    Ok(())
}

pub fn router() -> Router<Shared> {
    Router::new()
        .route(
            "/v1/devices",
            post(register)
                .layer(DefaultBodyLimit::max(4096))
                .get(list_devices),
        )
        .route("/v1/devices/{id}", delete(revoke))
        .route("/v1/state", get(state))
        .route("/v1/changes", get(changes))
        .route("/v1/messages/{id}/content", get(content))
        .route(
            "/v1/messages/{id}",
            put(commit)
                .layer(DefaultBodyLimit::max(MAX_TEXT))
                .delete(cancel),
        )
        .route("/v1/ack", post(ack).layer(DefaultBodyLimit::max(4096)))
}

fn canonical_uuid(s: &str) -> bool {
    Uuid::parse_str(s).is_ok_and(|v| v.to_string() == s && v.get_version_num() == 4)
}

fn token_valid(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())
}

fn hash(value: &[u8]) -> Vec<u8> {
    Sha256::digest(value).to_vec()
}

struct Device {
    id: String,
    token_hash: Vec<u8>,
}

fn check_device(db: &Connection, device: &Device) -> Result<(), StatusCode> {
    let stored: Option<Vec<u8>> = db
        .query_row(
            "SELECT token_hash FROM v1_devices WHERE id = ?1",
            [&device.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    match stored {
        Some(stored) if bool::from(stored.as_slice().ct_eq(&device.token_hash)) => Ok(()),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

// Extracted before a request body, so an unauthenticated caller cannot force
// the server to buffer a maximum-size clipboard value.
impl FromRequestParts<Shared> for Device {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, Self::Rejection> {
        let auth = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let (id, token) = auth.split_once(':').ok_or(StatusCode::UNAUTHORIZED)?;
        if !canonical_uuid(id) || !token_valid(token) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let device = Self {
            id: id.to_owned(),
            token_hash: hash(token.as_bytes()),
        };
        check_device(&*app.db.lock().await, &device)?;
        Ok(device)
    }
}

// Upload capacity is reserved before reading the request body. Waiting
// long-poll connections have a separate budget and cannot exhaust this one.
struct UploadPermit(#[allow(dead_code)] OwnedSemaphorePermit);

impl FromRequestParts<Shared> for UploadPermit {
    type Rejection = StatusCode;
    async fn from_request_parts(_: &mut Parts, app: &Shared) -> Result<Self, Self::Rejection> {
        Arc::clone(&app.v1_uploads)
            .try_acquire_owned()
            .map(Self)
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registration {
    device_id: String,
    device_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationResult {
    epoch: String,
    ack_revision: i64,
    current_revision: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    device_id: String,
    ack_revision: i64,
}

async fn list_devices(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    require(&headers, &app)?;
    let db = app.db.lock().await;
    let mut statement = db
        .prepare("SELECT id, ack_revision FROM v1_devices ORDER BY id")
        .map_err(db_error)?;
    let devices: Vec<DeviceInfo> = statement
        .query_map([], |r| {
            Ok(DeviceInfo {
                device_id: r.get(0)?,
                ack_revision: r.get(1)?,
            })
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(devices)))
}

async fn register(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<Registration>,
) -> Result<impl IntoResponse, StatusCode> {
    require(&headers, &app)?;
    if !canonical_uuid(&input.device_id) || !token_valid(&input.device_token) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut db = app.db.lock().await;
    let tx = db.transaction().map_err(db_error)?;
    let (epoch, current_revision): (String, i64) = tx
        .query_row(
            "SELECT epoch, revision FROM v1_meta WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(db_error)?;
    let existing: Option<(Vec<u8>, i64)> = tx
        .query_row(
            "SELECT token_hash, ack_revision FROM v1_devices WHERE id=?1",
            [&input.device_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    let token_hash = hash(input.device_token.as_bytes());
    let ack_revision = if let Some((saved, ack_revision)) = existing {
        if !bool::from(saved.as_slice().ct_eq(&token_hash)) {
            return Err(StatusCode::CONFLICT);
        }
        ack_revision
    } else {
        let count: i64 = tx
            .query_row("SELECT count(*) FROM v1_devices", [], |r| r.get(0))
            .map_err(db_error)?;
        if count >= MAX_DEVICES {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        tx.execute(
            "INSERT INTO v1_devices(id, token_hash, ack_revision) VALUES(?1, ?2, ?3)",
            params![input.device_id, token_hash, current_revision],
        )
        .map_err(db_error)?;
        current_revision
    };
    tx.commit().map_err(db_error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(RegistrationResult {
            epoch,
            ack_revision,
            current_revision,
        }),
    ))
}

async fn revoke(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    require(&headers, &app)?;
    if !canonical_uuid(&id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let removed = app
        .db
        .lock()
        .await
        .execute("DELETE FROM v1_devices WHERE id=?1", [&id])
        .map_err(db_error)?;
    if removed == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    let _ = app.v1_events.send(());
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentState {
    epoch: String,
    revision: i64,
    ack_revision: i64,
    message_id: Option<String>,
    source_device_id: Option<String>,
    created_at: Option<i64>,
    size: Option<i64>,
    sensitive: Option<bool>,
}

fn read_state(db: &Connection, device: &Device) -> Result<CurrentState, StatusCode> {
    check_device(db, device)?;
    db.query_row(
        "SELECT m.epoch, m.revision, d.ack_revision, e.message_id, e.device_id, e.created, length(e.body), e.sensitive
         FROM v1_meta m JOIN v1_devices d ON d.id=?1
         LEFT JOIN v1_messages e ON e.revision=m.revision WHERE m.singleton=1",
        [&device.id], |r| Ok(CurrentState { epoch:r.get(0)?, revision:r.get(1)?, ack_revision:r.get(2)?,
            message_id:r.get(3)?, source_device_id:r.get(4)?, created_at:r.get(5)?, size:r.get(6)?, sensitive:r.get(7)? }),
    ).map_err(db_error)
}

async fn state(State(app): State<Shared>, device: Device) -> Result<impl IntoResponse, StatusCode> {
    let current = read_state(&*app.db.lock().await, &device)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(current)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangesQuery {
    epoch: String,
    after: i64,
}

async fn changes(
    State(app): State<Shared>,
    device: Device,
    Query(query): Query<ChangesQuery>,
) -> Result<Response, StatusCode> {
    let _permit = app
        .v1_waiters
        .try_acquire()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // Subscribe before checking state to avoid missing a concurrent commit.
    let mut events = app.v1_events.subscribe();
    for attempt in 0..2 {
        let current = read_state(&*app.db.lock().await, &device)?;
        if query.epoch != current.epoch {
            return Err(StatusCode::CONFLICT);
        }
        if query.after < 0 || query.after > current.revision {
            return Err(StatusCode::BAD_REQUEST);
        }
        if current.revision > query.after {
            return Ok(([(header::CACHE_CONTROL, "no-store")], Json(current)).into_response());
        }
        if attempt == 0 {
            let _ = tokio::time::timeout(Duration::from_secs(25), events.recv()).await;
        }
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}

async fn content(
    State(app): State<Shared>,
    device: Device,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    if !canonical_uuid(&id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = app.db.lock().await;
    check_device(&db, &device)?;
    let bytes: Option<Vec<u8>> = db.query_row(
        "SELECT e.body FROM v1_messages e JOIN v1_meta m ON e.revision=m.revision WHERE e.message_id=?1",
        [&id], |r| r.get(0),
    ).optional().map_err(db_error)?;
    let bytes = bytes.ok_or(StatusCode::GONE)?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitResult {
    epoch: String,
    revision: i64,
    message_id: String,
    duplicate: bool,
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, StatusCode> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::BAD_REQUEST)
}

async fn commit(
    State(app): State<Shared>,
    device: Device,
    _permit: UploadPermit,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    if !canonical_uuid(&id) || std::str::from_utf8(&body).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let epoch = required_header(&headers, "x-rcs-epoch")?;
    let sensitive = match required_header(&headers, "x-rcs-sensitive")? {
        "0" => false,
        "1" => true,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let base: i64 = required_header(&headers, "x-rcs-base-revision")?
        .parse()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if base < 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut db = app.db.lock().await;
    let tx = db.transaction().map_err(db_error)?;
    check_device(&tx, &device)?;
    let (current_epoch, current_revision): (String, i64) = tx
        .query_row(
            "SELECT epoch, revision FROM v1_meta WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(db_error)?;
    if epoch != current_epoch {
        return Err(StatusCode::CONFLICT);
    }
    let cancelled: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM v1_cancelled WHERE message_id=?1)",
            [&id],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if cancelled {
        return Err(StatusCode::CONFLICT);
    }
    let body_hash = hash(&body);
    let existing: Option<(i64, String, i64, Vec<u8>, bool)> = tx.query_row(
        "SELECT revision, device_id, base_revision, body_hash, sensitive FROM v1_messages WHERE message_id=?1", [&id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    ).optional().map_err(db_error)?;
    if let Some((revision, owner, old_base, old_hash, old_sensitive)) = existing {
        if owner != device.id
            || old_base != base
            || old_sensitive != sensitive
            || !bool::from(old_hash.as_slice().ct_eq(&body_hash))
        {
            return Err(StatusCode::CONFLICT);
        }
        return Ok((
            [(header::CACHE_CONTROL, "no-store")],
            Json(CommitResult {
                epoch: current_epoch,
                revision,
                message_id: id,
                duplicate: true,
            }),
        ));
    }
    if base != current_revision {
        return Err(StatusCode::CONFLICT);
    }
    let revision = current_revision
        .checked_add(1)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    tx.execute("INSERT INTO v1_messages(revision, message_id, device_id, base_revision, body_hash, body, created, sensitive)
        VALUES(?1, ?2, ?3, ?4, ?5, ?6, unixepoch(), ?7)", params![revision, id, device.id, base, body_hash, body.as_ref(), sensitive]).map_err(db_error)?;
    tx.execute(
        "UPDATE v1_meta SET revision=?1 WHERE singleton=1",
        [revision],
    )
    .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    drop(db);
    let _ = app.v1_events.send(());
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(CommitResult {
            epoch: current_epoch,
            revision,
            message_id: id,
            duplicate: false,
        }),
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelResult {
    state: &'static str,
    revision: Option<i64>,
}

// Resolve an uncertain in-flight upload before replacing it with a newer local
// copy. The tombstone and commit are serialized by SQLite, so an old request
// arriving after cancellation cannot later publish its obsolete text.
async fn cancel(
    State(app): State<Shared>,
    device: Device,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    if !canonical_uuid(&id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let expected_epoch = required_header(&headers, "x-rcs-epoch")?;
    let mut db = app.db.lock().await;
    let tx = db.transaction().map_err(db_error)?;
    check_device(&tx, &device)?;
    let epoch: String = tx
        .query_row("SELECT epoch FROM v1_meta WHERE singleton=1", [], |r| {
            r.get(0)
        })
        .map_err(db_error)?;
    if expected_epoch != epoch {
        return Err(StatusCode::CONFLICT);
    }
    let committed: Option<(String, i64)> = tx
        .query_row(
            "SELECT device_id, revision FROM v1_messages WHERE message_id=?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    if let Some((owner, revision)) = committed {
        if owner != device.id {
            return Err(StatusCode::CONFLICT);
        }
        return Ok((
            [(header::CACHE_CONTROL, "no-store")],
            Json(CancelResult {
                state: "committed",
                revision: Some(revision),
            }),
        ));
    }
    let cancelled_owner: Option<String> = tx
        .query_row(
            "SELECT device_id FROM v1_cancelled WHERE message_id=?1",
            [&id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if cancelled_owner
        .as_ref()
        .is_some_and(|owner| owner != &device.id)
    {
        return Err(StatusCode::CONFLICT);
    }
    if cancelled_owner.is_none() {
        tx.execute(
            "INSERT INTO v1_cancelled(message_id, device_id, created) VALUES(?1, ?2, unixepoch())",
            params![id, device.id],
        )
        .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(CancelResult {
            state: "cancelled",
            revision: None,
        }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ack {
    epoch: String,
    revision: i64,
    message_id: String,
    result: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AckResult {
    ack_revision: i64,
}

async fn ack(
    State(app): State<Shared>,
    device: Device,
    Json(input): Json<Ack>,
) -> Result<impl IntoResponse, StatusCode> {
    if !canonical_uuid(&input.message_id)
        || !matches!(input.result.as_str(), "applied" | "self" | "skipped")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut db = app.db.lock().await;
    let tx = db.transaction().map_err(db_error)?;
    check_device(&tx, &device)?;
    let epoch: String = tx
        .query_row("SELECT epoch FROM v1_meta WHERE singleton=1", [], |r| {
            r.get(0)
        })
        .map_err(db_error)?;
    if epoch != input.epoch {
        return Err(StatusCode::CONFLICT);
    }
    let owner: Option<String> = tx
        .query_row(
            "SELECT device_id FROM v1_messages WHERE revision=?1 AND message_id=?2",
            params![input.revision, input.message_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let owner = owner.ok_or(StatusCode::GONE)?;
    if input.result == "self" && owner != device.id {
        return Err(StatusCode::BAD_REQUEST);
    }
    tx.execute(
        "UPDATE v1_devices SET ack_revision=max(ack_revision, ?1) WHERE id=?2",
        params![input.revision, device.id],
    )
    .map_err(db_error)?;
    let ack_revision: i64 = tx
        .query_row(
            "SELECT ack_revision FROM v1_devices WHERE id=?1",
            [&device.id],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(AckResult { ack_revision }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Mutex, Semaphore, broadcast};
    use tower::ServiceExt;

    fn app() -> (Shared, Router) {
        let db = Connection::open_in_memory().unwrap();
        init(&db).unwrap();
        let (v1_events, _) = broadcast::channel(16);
        let app = Arc::new(crate::App {
            db: Mutex::new(db),
            user: "test".into(),
            password: "long-password".into(),
            v1_events,
            v1_waiters: Arc::new(Semaphore::new(8)),
            v1_uploads: Arc::new(Semaphore::new(2)),
        });
        (Arc::clone(&app), crate::router(app))
    }

    async fn request(
        app: &Router,
        method: Method,
        url: &str,
        auth: &str,
        body: impl Into<Body>,
    ) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(url)
                    .header(header::AUTHORIZATION, auth)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body.into())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn json_body(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), 2 * MAX_TEXT).await.unwrap())
            .unwrap()
    }

    async fn register_device(app: &Router) -> (String, String, Value) {
        let id = Uuid::new_v4().to_string();
        let token = "ab".repeat(32);
        let basic = format!("Basic {}", STANDARD.encode("test:long-password"));
        let body = json!({"deviceId": id, "deviceToken": token}).to_string();
        let response = request(app, Method::POST, "/v1/devices", &basic, body).await;
        assert_eq!(response.status(), StatusCode::OK);
        (
            id.clone(),
            format!("Bearer {id}:{token}"),
            json_body(response).await,
        )
    }

    #[tokio::test]
    async fn service_exposes_only_health_and_v1_routes() {
        let (_, app) = app();
        let basic = format!("Basic {}", STANDARD.encode("test:long-password"));
        assert_eq!(
            request(&app, Method::GET, "/healthz", "", Body::empty())
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            request(
                &app,
                Method::GET,
                "/SyncClipboard.json",
                &basic,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(
                &app,
                Method::POST,
                "/SyncClipboardHub/negotiate",
                &basic,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn sensitive_marker_is_persisted_and_part_of_idempotency() {
        let (_, app) = app();
        let (_, auth, reg) = register_device(&app).await;
        let epoch = reg["epoch"].as_str().unwrap();
        let id = Uuid::new_v4().to_string();
        let make = |sensitive: Option<&str>| {
            let mut req = Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/messages/{id}"))
                .header(header::AUTHORIZATION, &auth)
                .header("x-rcs-epoch", epoch)
                .header("x-rcs-base-revision", "0");
            if let Some(value) = sensitive {
                req = req.header("x-rcs-sensitive", value);
            }
            req.body(Body::from("synthetic")).unwrap()
        };
        assert_eq!(
            app.clone().oneshot(make(None)).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            app.clone().oneshot(make(Some("1"))).await.unwrap().status(),
            StatusCode::OK
        );
        let state =
            json_body(request(&app, Method::GET, "/v1/state", &auth, Body::empty()).await).await;
        assert_eq!(state["sensitive"], true);
        assert_eq!(
            app.clone().oneshot(make(Some("0"))).await.unwrap().status(),
            StatusCode::CONFLICT
        );
    }

    async fn put_message(
        app: &Router,
        auth: &str,
        epoch: &str,
        base: i64,
        id: &str,
        text: &str,
    ) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/v1/messages/{id}"))
                    .header(header::AUTHORIZATION, auth)
                    .header("x-rcs-epoch", epoch)
                    .header("x-rcs-base-revision", base)
                    .header("x-rcs-sensitive", "0")
                    .body(Body::from(text.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn idempotent_delivery_baseline_ack_and_concurrent_conflicts() {
        let (_shared, app) = app();
        let (a_id, a, a_reg) = register_device(&app).await;
        let (_, b, b_reg) = register_device(&app).await;
        assert_eq!(
            request(&app, Method::GET, "/v1/devices", &a, Body::empty())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let basic = format!("Basic {}", STANDARD.encode("test:long-password"));
        let devices =
            json_body(request(&app, Method::GET, "/v1/devices", &basic, Body::empty()).await).await;
        assert_eq!(devices.as_array().unwrap().len(), 2);
        assert!(
            devices
                .as_array()
                .unwrap()
                .iter()
                .all(|v| v.get("deviceToken").is_none())
        );
        let epoch = a_reg["epoch"].as_str().unwrap();
        assert_eq!(a_reg["ackRevision"], 0);
        assert_eq!(b_reg["epoch"], epoch);
        let id1 = Uuid::new_v4().to_string();
        let first = put_message(&app, &a, epoch, 0, &id1, "first").await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(json_body(first).await["revision"], 1);
        let retry = put_message(&app, &a, epoch, 0, &id1, "first").await;
        assert_eq!(json_body(retry).await["duplicate"], true);
        assert_eq!(
            put_message(&app, &a, epoch, 0, &id1, "different")
                .await
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            put_message(&app, &b, epoch, 0, &id1, "first")
                .await
                .status(),
            StatusCode::CONFLICT
        );
        let (_, c, c_reg) = register_device(&app).await;
        assert_eq!(
            c_reg["ackRevision"], 1,
            "first pairing starts at current revision"
        );
        let observed =
            json_body(request(&app, Method::GET, "/v1/state", &b, Body::empty()).await).await;
        assert_eq!(observed["revision"], 1);
        assert_eq!(observed["sourceDeviceId"], a_id);
        assert_eq!(observed["messageId"], id1);
        let read = request(
            &app,
            Method::GET,
            &format!("/v1/messages/{id1}/content"),
            &b,
            Body::empty(),
        )
        .await;
        assert_eq!(to_bytes(read.into_body(), 32).await.unwrap(), "first");
        let ack_body =
            json!({"epoch":epoch,"revision":1,"messageId":id1,"result":"applied"}).to_string();
        assert_eq!(
            request(&app, Method::POST, "/v1/ack", &b, ack_body.clone())
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            json_body(request(&app, Method::POST, "/v1/ack", &b, ack_body).await).await["ackRevision"],
            1
        );
        let invalid_self =
            json!({"epoch":epoch,"revision":1,"messageId":id1,"result":"self"}).to_string();
        assert_eq!(
            request(&app, Method::POST, "/v1/ack", &b, invalid_self)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        let id2 = Uuid::new_v4().to_string();
        let id3 = Uuid::new_v4().to_string();
        let (left, right) = tokio::join!(
            put_message(&app, &a, epoch, 1, &id2, "new A"),
            put_message(&app, &b, epoch, 1, &id3, "new B"),
        );
        let mut statuses = [left.status().as_u16(), right.status().as_u16()];
        statuses.sort();
        assert_eq!(statuses, [200, 409]);
        assert_eq!(
            request(
                &app,
                Method::GET,
                &format!("/v1/messages/{id1}/content"),
                &c,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::GONE
        );
        assert_eq!(
            json_body(request(&app, Method::GET, "/v1/state", &b, Body::empty()).await).await["revision"],
            2
        );
    }

    #[tokio::test]
    async fn long_poll_wakes_after_commit_and_revocation_blocks_requests() {
        let (_, app) = app();
        let (_, a, reg) = register_device(&app).await;
        let (b_id, b, _) = register_device(&app).await;
        let epoch = reg["epoch"].as_str().unwrap().to_owned();
        let poll_url = format!("/v1/changes?epoch={epoch}&after=0");
        let poll = request(&app, Method::GET, &poll_url, &b, Body::empty());
        let writer = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            put_message(&app, &a, &epoch, 0, &Uuid::new_v4().to_string(), "wake").await
        };
        let (polled, written) = tokio::join!(poll, writer);
        assert_eq!(written.status(), StatusCode::OK);
        assert_eq!(json_body(polled).await["revision"], 1);
        let basic = format!("Basic {}", STANDARD.encode("test:long-password"));
        assert_eq!(
            request(
                &app,
                Method::DELETE,
                &format!("/v1/devices/{b_id}"),
                &basic,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            request(&app, Method::GET, "/v1/state", &b, Body::empty())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn rejects_unauthenticated_upload_before_reading_body() {
        let (_, app) = app();
        let polls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&polls);
        let stream = futures_util::stream::poll_fn(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(Bytes::from_static(b"secret"))))
        });
        let response = request(
            &app,
            Method::PUT,
            &format!("/v1/messages/{}", Uuid::new_v4()),
            "Bearer invalid",
            Body::from_stream(stream),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn history_expiry_preserves_current_and_recent_receipts() {
        let (shared, app) = app();
        let (_, a, reg) = register_device(&app).await;
        let epoch = reg["epoch"].as_str().unwrap();
        let old = Uuid::new_v4().to_string();
        let recent_receipt = Uuid::new_v4().to_string();
        let current = Uuid::new_v4().to_string();
        for (base, id) in [(0, &old), (1, &recent_receipt), (2, &current)] {
            assert_eq!(
                put_message(&app, &a, epoch, base, id, "text")
                    .await
                    .status(),
                StatusCode::OK
            );
        }
        {
            let db = shared.db.lock().await;
            db.execute(
                "UPDATE v1_messages SET created=unixepoch()-40*86400 WHERE message_id=?1",
                [&old],
            )
            .unwrap();
            db.execute(
                "UPDATE v1_messages SET created=unixepoch()-10*86400 WHERE message_id=?1",
                [&recent_receipt],
            )
            .unwrap();
            db.execute(
                "UPDATE v1_messages SET created=unixepoch()-40*86400 WHERE message_id=?1",
                [&current],
            )
            .unwrap();
            clean_history(&db, 7).unwrap();
            let retained: Vec<(String, bool)> = db
                .prepare("SELECT message_id, body IS NOT NULL FROM v1_messages ORDER BY revision")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert_eq!(
                retained,
                [(recent_receipt.clone(), false), (current.clone(), true)]
            );
        }
        let retry = put_message(&app, &a, epoch, 1, &recent_receipt, "text").await;
        assert_eq!(json_body(retry).await["duplicate"], true);
    }

    #[tokio::test]
    async fn cancelling_an_uncertain_upload_prevents_late_publication() {
        let (_, app) = app();
        let (_, a, reg) = register_device(&app).await;
        let epoch = reg["epoch"].as_str().unwrap();
        let obsolete = Uuid::new_v4().to_string();
        let cancel_request = || {
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/v1/messages/{obsolete}"))
                .header(header::AUTHORIZATION, &a)
                .header("x-rcs-epoch", epoch)
                .body(Body::empty())
                .unwrap()
        };
        let cancelled = app.clone().oneshot(cancel_request()).await.unwrap();
        assert_eq!(json_body(cancelled).await["state"], "cancelled");
        assert_eq!(
            json_body(app.clone().oneshot(cancel_request()).await.unwrap()).await["state"],
            "cancelled"
        );
        assert_eq!(
            put_message(&app, &a, epoch, 0, &obsolete, "obsolete")
                .await
                .status(),
            StatusCode::CONFLICT
        );
        let latest = Uuid::new_v4().to_string();
        assert_eq!(
            put_message(&app, &a, epoch, 0, &latest, "latest")
                .await
                .status(),
            StatusCode::OK
        );
        let cancel_committed = Request::builder()
            .method(Method::DELETE)
            .uri(format!("/v1/messages/{latest}"))
            .header(header::AUTHORIZATION, &a)
            .header("x-rcs-epoch", epoch)
            .body(Body::empty())
            .unwrap();
        let response = json_body(app.clone().oneshot(cancel_committed).await.unwrap()).await;
        assert_eq!(response["state"], "committed");
        assert_eq!(response["revision"], 1);
    }

    #[tokio::test]
    async fn restored_epoch_revokes_devices_and_retains_current_baseline() {
        let (shared, app) = app();
        let (_, a, reg) = register_device(&app).await;
        let epoch = reg["epoch"].as_str().unwrap();
        let id = Uuid::new_v4().to_string();
        assert_eq!(
            put_message(&app, &a, epoch, 0, &id, "old backup")
                .await
                .status(),
            StatusCode::OK
        );
        rotate_epoch(&mut *shared.db.lock().await).unwrap();
        assert_eq!(
            request(&app, Method::GET, "/v1/state", &a, Body::empty())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let (_, _, new_reg) = register_device(&app).await;
        assert_ne!(new_reg["epoch"], epoch);
        assert_eq!(new_reg["ackRevision"], 1);
    }

    #[tokio::test]
    async fn racing_cancel_and_commit_have_exactly_one_outcome() {
        for _ in 0..32 {
            let (_, app) = app();
            let (_, auth, reg) = register_device(&app).await;
            let epoch = reg["epoch"].as_str().unwrap();
            let id = Uuid::new_v4().to_string();
            let cancellation = Request::builder()
                .method(Method::DELETE)
                .uri(format!("/v1/messages/{id}"))
                .header(header::AUTHORIZATION, &auth)
                .header("x-rcs-epoch", epoch)
                .body(Body::empty())
                .unwrap();
            let (written, cancelled) = tokio::join!(
                put_message(&app, &auth, epoch, 0, &id, "synthetic"),
                app.clone().oneshot(cancellation),
            );
            let cancellation = json_body(cancelled.unwrap()).await;
            let current =
                json_body(request(&app, Method::GET, "/v1/state", &auth, Body::empty()).await)
                    .await;
            match cancellation["state"].as_str().unwrap() {
                "cancelled" => {
                    assert_eq!(written.status(), StatusCode::CONFLICT);
                    assert_eq!(current["revision"], 0);
                }
                "committed" => {
                    assert_eq!(written.status(), StatusCode::OK);
                    assert_eq!(current["revision"], 1);
                }
                _ => panic!("unexpected cancellation result"),
            }
        }
    }

    #[tokio::test]
    async fn v1_state_survives_encrypted_reopen_and_rejects_wrong_key() {
        let directory = std::env::temp_dir().join(format!("rcs-v1-test-{}", Uuid::new_v4()));
        let path = directory.join("clipsync.db");
        let key = "bc".repeat(32);
        let epoch;
        let id = Uuid::new_v4().to_string();
        {
            let db = crate::database::open(&path, &key).unwrap();
            let (v1_events, _) = broadcast::channel(16);
            let state = Arc::new(crate::App {
                db: Mutex::new(db),
                user: "test".into(),
                password: "long-password".into(),
                v1_events,
                v1_waiters: Arc::new(Semaphore::new(8)),
                v1_uploads: Arc::new(Semaphore::new(2)),
            });
            let app = crate::router(state);
            let (_, auth, reg) = register_device(&app).await;
            epoch = reg["epoch"].as_str().unwrap().to_owned();
            assert_eq!(
                put_message(&app, &auth, &epoch, 0, &id, "synthetic encrypted fixture")
                    .await
                    .status(),
                StatusCode::OK
            );
        }
        assert_ne!(&std::fs::read(&path).unwrap()[..16], b"SQLite format 3\0");
        assert!(crate::database::open(&path, &"dc".repeat(32)).is_err());
        let reopened = crate::database::open(&path, &key).unwrap();
        let result: (String, i64, String) = reopened.query_row(
            "SELECT m.epoch, m.revision, e.message_id FROM v1_meta m JOIN v1_messages e ON e.revision=m.revision WHERE m.singleton=1",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(result, (epoch, 1, id));
        drop(reopened);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
