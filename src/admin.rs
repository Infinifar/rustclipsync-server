//! Browser-only administration. Cookies never authorize the synchronization API.
use crate::{Shared, auth, db_error};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, FromRequestParts, Path, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

const COOKIE: &str = "__Host-rcs-admin";
const IDLE: Duration = Duration::from_secs(15 * 60);
const ABSOLUTE: Duration = Duration::from_secs(60 * 60);
const MAX_SESSIONS: usize = 16;
struct Session {
    created: Instant,
    active: Instant,
    csrf: String,
}
impl Session {
    fn valid_at(&self, now: Instant) -> bool {
        now.duration_since(self.created) < ABSOLUTE && now.duration_since(self.active) < IDLE
    }
}
#[derive(Default)]
pub struct WebState {
    origin: Option<String>,
    sessions: Mutex<HashMap<[u8; 32], Session>>,
}
impl WebState {
    pub fn enabled(origin: &str) -> Result<Self, &'static str> {
        let parsed =
            url::Url::parse(origin).map_err(|_| "RCS_WEB_ORIGIN must be an HTTPS origin")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err(
                "RCS_WEB_ORIGIN must be a single HTTPS origin without credentials, path or query",
            );
        }
        Ok(Self {
            origin: Some(parsed.origin().ascii_serialization()),
            sessions: Mutex::new(HashMap::new()),
        })
    }
    pub fn from_env(auth: &auth::AuthState) -> Result<Self, Box<dyn std::error::Error>> {
        match std::env::var("RCS_WEB_ENABLED")
            .unwrap_or_else(|_| "false".into())
            .as_str()
        {
            "false" | "0" => Ok(Self::default()),
            "true" | "1" => {
                if auth.admin.is_none() {
                    return Err(
                        "Web administration requires independent RCS_ADMIN_USER and password"
                            .into(),
                    );
                }
                Ok(Self::enabled(
                    &std::env::var("RCS_WEB_ORIGIN").map_err(|_| "RCS_WEB_ORIGIN is required")?,
                )?)
            }
            _ => Err("RCS_WEB_ENABLED must be true or false".into()),
        }
    }
    fn host(&self, headers: &HeaderMap) -> Result<&str, StatusCode> {
        let origin = self.origin.as_deref().ok_or(StatusCode::NOT_FOUND)?;
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::FORBIDDEN)?;
        if !host.eq_ignore_ascii_case(
            origin
                .strip_prefix("https://")
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?,
        ) {
            return Err(StatusCode::FORBIDDEN);
        }
        if headers
            .get("sec-fetch-site")
            .is_some_and(|v| v == "cross-site")
        {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(origin)
    }
    fn origin(&self, headers: &HeaderMap) -> Result<(), StatusCode> {
        let expected = self.host(headers)?;
        if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(expected) {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(())
    }
    fn authorize(&self, headers: &HeaderMap, mutation: bool) -> Result<[u8; 32], StatusCode> {
        self.host(headers)?;
        if mutation {
            self.origin(headers)?;
        }
        let key = cookie_key(headers)?;
        let now = Instant::now();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        sessions.retain(|_, s| s.valid_at(now));
        let session = sessions.get_mut(&key).ok_or(StatusCode::UNAUTHORIZED)?;
        if mutation {
            let csrf = headers
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .ok_or(StatusCode::FORBIDDEN)?;
            if !bool::from(csrf.as_bytes().ct_eq(session.csrf.as_bytes())) {
                return Err(StatusCode::FORBIDDEN);
            }
            session.active = now;
        }
        Ok(key)
    }
}
fn cookie_key(headers: &HeaderMap) -> Result<[u8; 32], StatusCode> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        let value = header.to_str().map_err(|_| StatusCode::UNAUTHORIZED)?;
        for part in value.split(';') {
            if let Some((name, value)) = part.trim().split_once('=')
                && name == COOKIE
            {
                if found.is_some()
                    || value.len() != 64
                    || !value.bytes().all(|c| c.is_ascii_hexdigit())
                {
                    return Err(StatusCode::UNAUTHORIZED);
                }
                found = Some(Sha256::digest(value.as_bytes()).into());
            }
        }
    }
    found.ok_or(StatusCode::UNAUTHORIZED)
}
struct Admin([u8; 32]);
impl FromRequestParts<Shared> for Admin {
    type Rejection = StatusCode;
    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, StatusCode> {
        app.web
            .authorize(
                &parts.headers,
                !matches!(
                    parts.method,
                    axum::http::Method::GET | axum::http::Method::HEAD
                ),
            )
            .map(Self)
    }
}
pub fn router() -> Router<Shared> {
    Router::new()
        .route("/admin", get(page))
        .route("/admin/", get(page))
        .route("/admin/admin.css", get(css))
        .route("/admin/admin.js", get(js))
        .route("/api/admin/login", post(login))
        .route("/api/admin/session", get(session).post(session))
        .route("/api/admin/logout", post(logout))
        .route("/api/admin/devices", get(devices))
        .route("/api/admin/about", get(about))
        .route("/api/admin/diagnostics/reset", post(reset_diagnostics))
        .route(
            "/api/admin/devices/{id}",
            delete(revoke_device).patch(rename_device),
        )
        .route(
            "/api/admin/sync-settings",
            get(sync_settings).put(save_sync_settings),
        )
        .route("/api/admin/accounts", get(accounts).post(create_account))
        .route("/api/admin/accounts/{id}", delete(revoke_account))
        // A 4096-byte password can expand sixfold when escaped in JSON.
        .layer(DefaultBodyLimit::max(32768))
        .layer(middleware::from_fn(security_headers))
}
async fn security_headers(request: axum::http::Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        ("cache-control", "no-store"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self' https://api.github.com; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
        ("x-frame-options", "DENY"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            axum::http::HeaderValue::from_static(value),
        );
    }
    response
}
async fn page(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    app.web.host(&headers)?;
    Ok(axum::response::Html(include_str!("../assets/admin.html")))
}
async fn css(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    app.web.host(&headers)?;
    Ok((
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../assets/admin.css"),
    ))
}
async fn js(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    app.web.host(&headers)?;
    Ok((
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../assets/admin.js"),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    username: String,
    password: String,
}
async fn login(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<Response, StatusCode> {
    app.web.origin(&headers)?;
    if headers.get("x-rcs-admin").and_then(|v| v.to_str().ok()) != Some("1") {
        return Err(StatusCode::FORBIDDEN);
    }
    app.auth.attempt()?;
    let admin = app.auth.admin.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    if !admin.matches(&input.username, &input.password) {
        eprintln!("admin login rejected");
        return Err(StatusCode::UNAUTHORIZED);
    }
    let token = auth::random_hex()?;
    let csrf = auth::random_hex()?;
    let key: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let now = Instant::now();
    {
        let mut sessions = app
            .web
            .sessions
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        sessions.retain(|_, s| s.valid_at(now));
        if let Ok(old) = cookie_key(&headers) {
            sessions.remove(&old);
        }
        if sessions.len() >= MAX_SESSIONS
            && let Some(old) = sessions
                .iter()
                .min_by_key(|(_, s)| s.created)
                .map(|(k, _)| *k)
        {
            sessions.remove(&old);
        }
        sessions.insert(
            key,
            Session {
                created: now,
                active: now,
                csrf: csrf.clone(),
            },
        );
    }
    let mut response=Json(serde_json::json!({"username":admin.user,"csrfToken":csrf,"idleSeconds":IDLE.as_secs(),"absoluteSeconds":ABSOLUTE.as_secs()})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("{COOKIE}={token}; Secure; HttpOnly; SameSite=Strict; Path=/")
            .parse()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    eprintln!("admin login succeeded");
    Ok(response)
}
async fn session(
    State(app): State<Shared>,
    Admin(key): Admin,
) -> Result<impl IntoResponse, StatusCode> {
    let sessions = app
        .web
        .sessions
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let session = sessions.get(&key).ok_or(StatusCode::UNAUTHORIZED)?;
    let user = app.auth.admin.as_ref().ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(Json(
        serde_json::json!({"username":user.user,"csrfToken":session.csrf,"idleSeconds":IDLE.as_secs(),"absoluteSeconds":ABSOLUTE.as_secs()}),
    ))
}
async fn logout(State(app): State<Shared>, Admin(key): Admin) -> Result<Response, StatusCode> {
    app.web
        .sessions
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .remove(&key);
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        axum::http::HeaderValue::from_static(
            "__Host-rcs-admin=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        ),
    );
    eprintln!("admin logout");
    Ok(response)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRow {
    device_id: String,
    label: String,
    account_id: Option<String>,
    account_name: String,
    ack_revision: i64,
    online: bool,
    last_seen: Option<i64>,
    sync: DeviceSync,
}
async fn about(_: Admin) -> impl IntoResponse {
    Json(
        serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"protocolVersion":crate::direction::PROTOCOL}),
    )
}
#[derive(Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ReportState {
    Unknown,
    Reported,
    Revoked,
    Expired,
}
/// Read-only projection: acceptance and a data lease do not prove client health.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceSync {
    direction: Option<String>,
    paused: bool,
    report_state: ReportState,
    reported_at: Option<i64>,
    expires_at: Option<i64>,
    last_report_source: Option<&'static str>,
    version: Option<i64>,
    data_lease_active: bool,
}
impl DeviceSync {
    fn from_row(row: &rusqlite::Row<'_>, now: i64) -> rusqlite::Result<Self> {
        let direction: Option<String> = row.get(6)?;
        let paused: Option<bool> = row.get(7)?;
        let expires_at: Option<i64> = row.get(9)?;
        let lease: Option<i64> = row.get(10)?;
        let op: Option<String> = row.get(11)?;
        let source: Option<String> = row.get(12)?;
        let report_state = match op.as_deref() {
            Some("revoke") => ReportState::Revoked,
            Some("expired") => ReportState::Expired,
            _ if direction.is_some() && expires_at.is_some_and(|at| at <= now) => {
                ReportState::Expired
            }
            _ if matches!(
                direction.as_deref(),
                Some("PUSH_ONLY" | "RECEIVE_ONLY" | "BIDIRECTIONAL")
            ) && expires_at.is_some_and(|at| at > now) =>
            {
                ReportState::Reported
            }
            _ => ReportState::Unknown,
        };
        let active = report_state == ReportState::Reported;
        let paused = active && paused.unwrap_or(false);
        Ok(Self {
            direction: if active { direction } else { None },
            paused,
            report_state,
            reported_at: row.get(8)?,
            expires_at,
            last_report_source: match source.as_deref() {
                Some("WS") => Some("ws"),
                Some("HTTP") => Some("http"),
                _ => None,
            },
            version: row.get(13)?,
            data_lease_active: active && !paused && lease.is_some_and(|at| at > now),
        })
    }
}
async fn devices(State(app): State<Shared>, _: Admin) -> Result<impl IntoResponse, StatusCode> {
    let now = crate::presence::unix_time();
    let db = app.db.lock().await;
    let mut statement=db.prepare("SELECT d.id,d.label,d.account_id,a.username,d.ack_revision,d.last_seen,s.direction,s.paused,s.updated,s.expires,s.lease_until,s.last_op,s.source,s.version FROM v1_devices d LEFT JOIN rcs_accounts a ON a.id=d.account_id LEFT JOIN rcs_direction s ON s.device_id=d.id ORDER BY d.id").map_err(db_error)?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
                DeviceSync::from_row(r, now)?,
            ))
        })
        .map_err(db_error)?;
    let mut devices = Vec::new();
    for row in rows {
        let (id, label, account_id, name, ack, last, sync) = row.map_err(db_error)?;
        let name = if account_id.as_deref() == Some(auth::ADMIN_ACCOUNT) {
            app.auth
                .admin
                .as_ref()
                .map(|a| a.user.clone())
                .unwrap_or_else(|| "Administrator".into())
        } else {
            name.unwrap_or_else(|| app.user.clone())
        };
        let (online, last_seen) = app.presence.status(&id, last)?;
        devices.push(DeviceRow {
            device_id: id,
            label,
            account_id,
            account_name: name,
            ack_revision: ack,
            online,
            last_seen,
            sync,
        });
    }
    Ok(Json(devices))
}
fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|u| u.get_version_num() == 4 && u.to_string() == id)
}
async fn revoke_device(
    State(app): State<Shared>,
    _: Admin,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    if !valid_id(&id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let count = app
        .db
        .lock()
        .await
        .execute("DELETE FROM v1_devices WHERE id=?1", [&id])
        .map_err(db_error)?;
    if count == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    let _ = app.events.send(());
    eprintln!("admin revoked device {id}");
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rename {
    label: String,
}
async fn rename_device(
    State(app): State<Shared>,
    _: Admin,
    Path(id): Path<String>,
    Json(input): Json<Rename>,
) -> Result<StatusCode, StatusCode> {
    if !valid_id(&id)
        || input.label.chars().count() > 80
        || input.label.len() > 320
        || input.label.chars().any(char::is_control)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let count = app
        .db
        .lock()
        .await
        .execute(
            "UPDATE v1_devices SET label=?1 WHERE id=?2",
            params![input.label.trim(), id],
        )
        .map_err(db_error)?;
    if count == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountRow {
    id: String,
    username: String,
    kind: &'static str,
    devices: i64,
}
async fn accounts(State(app): State<Shared>, _: Admin) -> Result<impl IntoResponse, StatusCode> {
    let db = app.db.lock().await;
    let mut statement=db.prepare("SELECT a.id,a.username,count(d.id) FROM rcs_accounts a LEFT JOIN v1_devices d ON d.account_id=a.id GROUP BY a.id ORDER BY a.username").map_err(db_error)?;
    let rows = statement
        .query_map([], |r| {
            Ok(AccountRow {
                id: r.get(0)?,
                username: r.get(1)?,
                kind: "device",
                devices: r.get(2)?,
            })
        })
        .map_err(db_error)?;
    let mut accounts = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for (id, user) in [
        (None, app.user.clone()),
        (
            Some(auth::ADMIN_ACCOUNT),
            app.auth
                .admin
                .as_ref()
                .map(|a| a.user.clone())
                .unwrap_or_default(),
        ),
    ] {
        let count = db
            .query_row(
                "SELECT count(*) FROM v1_devices WHERE account_id IS ?1",
                [id],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        accounts.push(AccountRow {
            id: id.unwrap_or("@legacy").into(),
            username: user,
            kind: if id.is_some() { "admin" } else { "legacy" },
            devices: count,
        });
    }
    Ok(Json(accounts))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewAccount {
    username: String,
    password: String,
}
async fn create_account(
    State(app): State<Shared>,
    _: Admin,
    Json(input): Json<NewAccount>,
) -> Result<impl IntoResponse, StatusCode> {
    if input.username.is_empty()
        || input.username.len() > 64
        || !input
            .username
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    if input.username == app.user
        || app
            .auth
            .admin
            .as_ref()
            .is_some_and(|a| a.user == input.username)
    {
        return Err(StatusCode::CONFLICT);
    }
    let hash = auth::password_hash(&app, input.password).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut db = app.db.lock().await;
    let tx = db.transaction().map_err(db_error)?;
    let count: i64 = tx
        .query_row("SELECT count(*) FROM rcs_accounts", [], |r| r.get(0))
        .map_err(db_error)?;
    if count >= auth::MAX_ACCOUNTS {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rcs_accounts WHERE username=?1)",
            [&input.username],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if exists {
        return Err(StatusCode::CONFLICT);
    }
    tx.execute(
        "INSERT INTO rcs_accounts(id,username,password_hash) VALUES(?1,?2,?3)",
        params![id, input.username, hash],
    )
    .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    eprintln!("admin created device account {id}");
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id":id,"username":input.username})),
    ))
}
async fn revoke_account(
    State(app): State<Shared>,
    _: Admin,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    if !valid_id(&id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut db = app.db.lock().await;
    let tx = db.transaction().map_err(db_error)?;
    let exists: Option<String> = tx
        .query_row("SELECT id FROM rcs_accounts WHERE id=?1", [&id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(db_error)?;
    if exists.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let count = tx
        .execute("DELETE FROM v1_devices WHERE account_id=?1", [&id])
        .map_err(db_error)?;
    tx.execute("DELETE FROM rcs_accounts WHERE id=?1", [&id])
        .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    drop(db);
    let _ = app.events.send(());
    eprintln!("admin revoked account {id}, devices={count}");
    Ok(Json(serde_json::json!({"revokedDevices":count})))
}

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncSettingsInput {
    backfill_window_minutes: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetDiagnosticsInput {
    confirm: bool,
}
async fn reset_diagnostics(
    State(app): State<Shared>,
    _: Admin,
    Json(input): Json<ResetDiagnosticsInput>,
) -> Result<impl IntoResponse, StatusCode> {
    if !input.confirm {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Serialize with normal routing updates/snapshots; no database writes occur.
    let db = app.db.lock().await;
    crate::direction::reset_statistics(&app.direction);
    let metrics = crate::direction::statistics(&db, &app.direction);
    tracing::info!(event = "sync.diagnostics.reset");
    Ok(Json(serde_json::json!({"metrics":metrics})))
}
async fn sync_settings(
    State(app): State<Shared>,
    _: Admin,
) -> Result<impl IntoResponse, StatusCode> {
    let db = app.db.lock().await;
    let valid = db
        .query_row(
            "SELECT value FROM rcs_settings WHERE key='sync.reconnect.backfill.window'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .is_ok_and(|n| (0..=1440).contains(&n));
    let mut stmt=db.prepare("SELECT actor,changed,old_value,new_value FROM rcs_settings_audit ORDER BY id DESC LIMIT 20").map_err(db_error)?;
    let audit=stmt.query_map([],|r|Ok(serde_json::json!({"actor":r.get::<_,String>(0)?,"changedAt":r.get::<_,i64>(1)?,"oldValue":r.get::<_,Option<i64>>(2)?,"newValue":r.get::<_,i64>(3)?}))).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
    Ok(Json(
        serde_json::json!({"key":"sync.reconnect.backfill.window","backfillWindowMinutes":crate::direction::window(&db,&app.direction),"configurationValid":valid,"audit":audit,"metrics":crate::direction::statistics(&db,&app.direction)}),
    ))
}
async fn save_sync_settings(
    State(app): State<Shared>,
    _: Admin,
    Json(input): Json<SyncSettingsInput>,
) -> Result<StatusCode, StatusCode> {
    let actor = app
        .auth
        .admin
        .as_ref()
        .ok_or(StatusCode::UNAUTHORIZED)?
        .user
        .as_str();
    crate::direction::set_window(
        &mut *app.db.lock().await,
        &app.direction,
        actor,
        input.backfill_window_minutes,
    )?;
    let _ = app.events.send(());
    Ok(StatusCode::NO_CONTENT)
}
