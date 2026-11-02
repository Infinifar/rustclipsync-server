//! Device-scoped direction control, session fencing and bounded offline delivery.
use crate::{Shared, api::Device, db_error};
use axum::{
    Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

pub const PROTOCOL: &str = "3";
pub const TTL: i64 = 30 * 86400;
const LEASE: i64 = 30;
const MAX_VERSION: i64 = 9_007_199_254_740_991;
const KEY: &str = "sync.reconnect.backfill.window";
#[derive(Default)]
pub struct Metrics {
    reports: AtomicU64,
    errors: AtomicU64,
    ws: AtomicU64,
    http: AtomicU64,
    filtered: AtomicU64,
    unknown: AtomicU64,
    revoked: AtomicU64,
    expired: AtomicU64,
    backfills: AtomicU64,
    alerts: AtomicU64,
}
pub struct Runtime {
    pub sockets: Arc<Semaphore>,
    rates: Mutex<HashMap<String, (Instant, u32)>>,
    warned: AtomicBool,
    pub metrics: Metrics,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            sockets: Arc::new(Semaphore::new(32)),
            rates: Mutex::new(HashMap::new()),
            warned: AtomicBool::new(false),
            metrics: Metrics::default(),
        }
    }
}
impl Runtime {
    fn attempt(&self, id: &str) -> Result<(), Fault> {
        let mut rates = self.rates.lock().map_err(|_| Fault::internal())?;
        rates.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
        if !rates.contains_key(id) && rates.len() >= 64 {
            return Err(Fault::rate());
        }
        let (_, count) = rates.entry(id.into()).or_insert((Instant::now(), 0));
        if *count >= 120 {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return Err(Fault::rate());
        }
        *count += 1;
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SyncDirection {
    PushOnly,
    ReceiveOnly,
    Bidirectional,
}
impl SyncDirection {
    fn text(self) -> &'static str {
        match self {
            Self::PushOnly => "PUSH_ONLY",
            Self::ReceiveOnly => "RECEIVE_ONLY",
            Self::Bidirectional => "BIDIRECTIONAL",
        }
    }
}
#[derive(Debug)]
pub struct Fault {
    pub status: StatusCode,
    pub code: &'static str,
    pub version: Option<i64>,
}
impl Fault {
    pub(crate) fn new(status: StatusCode, code: &'static str) -> Self {
        Self {
            status,
            code,
            version: None,
        }
    }
    fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
    }
    fn stale(version: i64) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "VERSION_STALE",
            version: Some(version),
        }
    }
    fn revoked() -> Self {
        Self::new(StatusCode::CONFLICT, "SESSION_REVOKED")
    }
    fn invalid() -> Self {
        Self::new(StatusCode::BAD_REQUEST, "INVALID_DIRECTION")
    }
    fn rate() -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED")
    }
}
impl From<StatusCode> for Fault {
    fn from(status: StatusCode) -> Self {
        Self::new(
            status,
            match status {
                StatusCode::UNAUTHORIZED => "UNAUTHENTICATED",
                StatusCode::FORBIDDEN => "DIRECTION_FORBIDDEN",
                StatusCode::CONFLICT => "CONFLICT",
                StatusCode::GONE => "MESSAGE_GONE",
                StatusCode::BAD_REQUEST => "INVALID_REQUEST",
                _ => "REQUEST_FAILED",
            },
        )
    }
}
impl From<rusqlite::Error> for Fault {
    fn from(_: rusqlite::Error) -> Self {
        Self::internal()
    }
}
impl IntoResponse for Fault {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({"code":self.code,"version":self.version})),
        )
            .into_response()
    }
}

pub fn init(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS rcs_direction(
          device_id TEXT PRIMARY KEY REFERENCES v1_devices(id) ON DELETE CASCADE,
          session_id TEXT,version INTEGER NOT NULL DEFAULT 0,direction TEXT,paused INTEGER NOT NULL DEFAULT 0,
          updated INTEGER,expires INTEGER,lease_until INTEGER NOT NULL DEFAULT 0,
          last_op TEXT NOT NULL DEFAULT '',last_payload TEXT NOT NULL DEFAULT '',source TEXT NOT NULL DEFAULT 'HTTP',issued_revision INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS rcs_delivery(
          device_id TEXT PRIMARY KEY REFERENCES v1_devices(id) ON DELETE CASCADE,
          message_id TEXT NOT NULL,revision INTEGER NOT NULL,session_id TEXT NOT NULL,expires INTEGER,backfill INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS rcs_backfill(
          device_id TEXT PRIMARY KEY REFERENCES v1_devices(id) ON DELETE CASCADE,
          message_id TEXT NOT NULL,revision INTEGER NOT NULL,expires INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS rcs_settings(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS rcs_settings_initialized(id INTEGER PRIMARY KEY CHECK(id=1));
        CREATE TABLE IF NOT EXISTS rcs_settings_audit(id INTEGER PRIMARY KEY,actor TEXT NOT NULL,changed INTEGER NOT NULL,old_value INTEGER,new_value INTEGER NOT NULL);
        INSERT OR IGNORE INTO rcs_settings(key,value) SELECT 'sync.reconnect.backfill.window',5 WHERE NOT EXISTS(SELECT 1 FROM rcs_settings_initialized);
        INSERT OR IGNORE INTO rcs_settings_initialized VALUES(1);")?;
    for (table, column, definition) in [
        (
            "rcs_direction",
            "issued_revision",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("rcs_delivery", "expires", "INTEGER"),
        ("rcs_delivery", "backfill", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
            params![table, column],
            |r| r.get(0),
        )?;
        if !exists {
            db.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    Ok(())
}
/// A restarted process owns no previous online connections; fixed-TTL offline
/// candidates survive, but live delivery requires a new direction handshake.
pub fn restart(db: &mut Connection) -> rusqlite::Result<()> {
    let tx = db.transaction()?;
    tx.execute("UPDATE rcs_direction SET lease_until=0", [])?;
    tx.execute("DELETE FROM rcs_delivery", [])?;
    tx.commit()
}
pub fn window(db: &Connection, rt: &Runtime) -> u16 {
    let value = db.query_row("SELECT value FROM rcs_settings WHERE key=?1", [KEY], |r| {
        r.get::<_, i64>(0)
    });
    match value {
        Ok(n) if (0..=1440).contains(&n) => {
            rt.warned.store(false, Ordering::Relaxed);
            n as u16
        }
        _ => {
            rt.metrics.alerts.fetch_add(1, Ordering::Relaxed);
            let _ = db.execute("DELETE FROM rcs_backfill", []);
            let _ = db.execute("DELETE FROM rcs_delivery WHERE backfill=1", []);
            if !rt.warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    event = "sync.config.invalid",
                    effective_window_minutes = 0,
                    "Reconnect backfill disabled by invalid configuration"
                );
            }
            0
        }
    }
}
pub fn set_window(
    db: &mut Connection,
    rt: &Runtime,
    actor: &str,
    value: i64,
) -> Result<(), StatusCode> {
    if !(0..=1440).contains(&value) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let tx = db.transaction().map_err(db_error)?;
    let old = tx
        .query_row("SELECT value FROM rcs_settings WHERE key=?1", [KEY], |r| {
            r.get::<_, rusqlite::types::Value>(0)
        })
        .optional()
        .map_err(db_error)?
        .and_then(|value| match value {
            rusqlite::types::Value::Integer(n) => Some(n),
            _ => None,
        });
    tx.execute("INSERT INTO rcs_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![KEY,value]).map_err(db_error)?;
    tx.execute("INSERT INTO rcs_settings_audit(actor,changed,old_value,new_value) VALUES(?1,unixepoch(),?2,?3)",params![actor,old,value]).map_err(db_error)?;
    if value == 0 {
        tx.execute("DELETE FROM rcs_backfill", [])
            .map_err(db_error)?;
        tx.execute("DELETE FROM rcs_delivery WHERE backfill=1", [])
            .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;
    rt.warned.store(false, Ordering::Relaxed);
    tracing::info!(event="sync.config.changed",actor,old_window_minutes=?old,new_window_minutes=value);
    Ok(())
}
pub fn statistics(db: &Connection, rt: &Runtime) -> serde_json::Value {
    let m = &rt.metrics;
    serde_json::json!({"reports":m.reports.load(Ordering::Relaxed),"reportErrors":m.errors.load(Ordering::Relaxed),"wsReports":m.ws.load(Ordering::Relaxed),"httpReports":m.http.load(Ordering::Relaxed),"filtered":m.filtered.load(Ordering::Relaxed),"unknown":m.unknown.load(Ordering::Relaxed),"revocations":m.revoked.load(Ordering::Relaxed),"expired":m.expired.load(Ordering::Relaxed),"backfills":m.backfills.load(Ordering::Relaxed),"configurationAlerts":m.alerts.load(Ordering::Relaxed),"backfillWindowMinutes":window(db,rt)})
}
pub fn expire(db: &Connection, rt: &Runtime, now: i64) -> rusqlite::Result<()> {
    // Validate policy on read paths too: corrupt configuration must immediately
    // disable already-promoted offline candidates, not only future writes.
    let _ = window(db, rt);
    db.execute("DELETE FROM rcs_delivery WHERE device_id IN(SELECT device_id FROM rcs_direction WHERE (direction IS NOT NULL AND expires<=?1) OR lease_until<=?1) OR expires<=?1",[now])?;
    db.execute("DELETE FROM rcs_backfill WHERE expires<=?1 OR device_id IN(SELECT device_id FROM rcs_direction WHERE direction IS NOT NULL AND expires<=?1)",[now])?;
    let count=db.execute("UPDATE rcs_direction SET direction=NULL,paused=0,lease_until=0,last_op='expired' WHERE direction IS NOT NULL AND expires<=?1",[now])?;
    rt.metrics
        .expired
        .fetch_add(count as u64, Ordering::Relaxed);
    Ok(())
}
pub fn touch(db: &Connection, rt: &Runtime, id: &str, now: i64) -> rusqlite::Result<()> {
    expire(db, rt, now)?;
    // Throttle durable TTL refresh to once per minute, independently of reads.
    db.execute("UPDATE rcs_direction SET expires=?2 WHERE device_id=?1 AND direction IS NOT NULL AND expires>?2-?3 AND expires<?2-60",params![id,now+TTL,TTL])?;
    Ok(())
}
#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Update {
    pub session_id: String,
    pub version: i64,
    pub direction: SyncDirection,
    pub paused: bool,
    pub connect: bool,
}
#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Revoke {
    pub session_id: String,
    pub version: i64,
}
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Acknowledged {
    device_id: String,
    session_id: String,
    version: i64,
    server_time: i64,
    expires_at: Option<i64>,
    revoked: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    device_id: String,
    session_id: Option<String>,
    version: i64,
    direction: Option<String>,
    paused: bool,
    updated_at: Option<i64>,
    expires_at: Option<i64>,
    revoked: bool,
}
fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|u| u.get_version_num() == 4 && u.to_string() == id)
}
fn ownership(device: &Device, id: &str) -> Result<(), Fault> {
    if device.id == id {
        Ok(())
    } else {
        Err(Fault::new(StatusCode::FORBIDDEN, "DEVICE_MISMATCH"))
    }
}
fn ensure_device(db: &Connection, device: &Device) -> Result<(), Fault> {
    crate::api::check_device(db, device).map_err(|s| Fault::new(s, "UNAUTHENTICATED"))
}
fn snapshot(db: &Connection, id: &str) -> Result<Snapshot, Fault> {
    Ok(db.query_row("SELECT session_id,version,direction,paused,updated,expires,last_op FROM rcs_direction WHERE device_id=?1",[id],|r|Ok(Snapshot{device_id:id.into(),session_id:r.get(0)?,version:r.get(1)?,direction:r.get(2)?,paused:r.get(3)?,updated_at:r.get(4)?,expires_at:r.get(5)?,revoked:matches!(r.get::<_,String>(6)?.as_str(),"revoke"|"expired")})).optional()?.unwrap_or(Snapshot{device_id:id.into(),session_id:None,version:0,direction:None,paused:false,updated_at:None,expires_at:None,revoked:false}))
}
fn mutate(
    db: &mut Connection,
    rt: &Runtime,
    device: &Device,
    update: Option<&Update>,
    revoke: Option<&Revoke>,
    source: &str,
    now: i64,
) -> Result<Acknowledged, Fault> {
    rt.attempt(&device.id)?;
    let (sid, version, op, payload) = if let Some(u) = update {
        (
            &u.session_id,
            u.version,
            "update",
            serde_json::to_string(u).map_err(|_| Fault::invalid())?,
        )
    } else if let Some(r) = revoke {
        (
            &r.session_id,
            r.version,
            "revoke",
            serde_json::to_string(r).map_err(|_| Fault::invalid())?,
        )
    } else {
        return Err(Fault::invalid());
    };
    if !valid_id(sid) || !(1..=MAX_VERSION).contains(&version) {
        return Err(Fault::invalid());
    }
    let tx = db.transaction()?;
    ensure_device(&tx, device)?;
    expire(&tx, rt, now)?;
    let old = snapshot(&tx, &device.id)?;
    let last: Option<(String, String)> = tx
        .query_row(
            "SELECT last_op,last_payload FROM rcs_direction WHERE device_id=?1",
            [&device.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if old.version == version && last == Some((op.into(), payload.clone())) {
        if op == "update" && old.direction.is_none() {
            return Err(Fault::revoked());
        }
        tx.execute("UPDATE rcs_direction SET expires=CASE WHEN direction IS NOT NULL THEN ?2 ELSE expires END,lease_until=CASE WHEN direction IS NOT NULL AND paused=0 THEN ?3 ELSE 0 END,source=?4 WHERE device_id=?1",params![device.id,now+TTL,now+LEASE,source])?;
        if op == "update"
            && !old.paused
            && old.direction.as_deref().is_some_and(|d| d != "PUSH_ONLY")
            && window(&tx, rt) > 0
        {
            tx.execute("INSERT OR IGNORE INTO rcs_delivery(device_id,message_id,revision,session_id,expires,backfill) SELECT device_id,message_id,revision,?2,expires,1 FROM rcs_backfill WHERE device_id=?1 AND expires>?3 AND revision>(SELECT ack_revision FROM v1_devices WHERE id=?1)",params![device.id,sid,now])?;
        }
        tx.execute("UPDATE rcs_direction SET issued_revision=max(issued_revision,coalesce((SELECT revision FROM rcs_delivery WHERE device_id=?1),0)) WHERE device_id=?1",[&device.id])?;
        tx.commit()?;
        return Ok(Acknowledged {
            device_id: device.id.clone(),
            session_id: sid.clone(),
            version,
            server_time: now,
            expires_at: if old.direction.is_some() {
                Some(now + TTL)
            } else {
                None
            },
            revoked: op == "revoke",
        });
    }
    if version != old.version.checked_add(1).ok_or_else(Fault::internal)? {
        return Err(Fault::stale(old.version));
    }
    let connect = update.is_some_and(|u| u.connect);
    if !connect && (old.direction.is_none() || old.session_id.as_deref() != Some(sid)) {
        return Err(Fault::revoked());
    }
    if connect && old.session_id.as_deref() == Some(sid) {
        return Err(Fault::stale(old.version));
    }
    let direction = update.map(|u| u.direction.text());
    let paused = update.is_some_and(|u| u.paused);
    if connect || paused || update.is_none() {
        tx.execute("DELETE FROM rcs_delivery WHERE device_id=?1", [&device.id])?;
    }
    if paused || update.is_none() || update.is_some_and(|u| u.direction == SyncDirection::PushOnly)
    {
        tx.execute("DELETE FROM rcs_backfill WHERE device_id=?1", [&device.id])?;
        tx.execute("DELETE FROM rcs_delivery WHERE device_id=?1", [&device.id])?;
    }
    tx.execute("INSERT INTO rcs_direction(device_id,session_id,version,direction,paused,updated,expires,lease_until,last_op,last_payload,source) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(device_id) DO UPDATE SET session_id=excluded.session_id,version=excluded.version,direction=excluded.direction,paused=excluded.paused,updated=excluded.updated,expires=excluded.expires,lease_until=excluded.lease_until,last_op=excluded.last_op,last_payload=excluded.last_payload,source=excluded.source",params![device.id,sid,version,direction,paused,now,update.map(|_|now+TTL),if update.is_some()&&!paused{now+LEASE}else{0},op,payload,source])?;
    if connect && !paused && direction.is_some_and(|d| d != "PUSH_ONLY") && window(&tx, rt) > 0 {
        let moved=tx.execute("INSERT OR REPLACE INTO rcs_delivery(device_id,message_id,revision,session_id,expires,backfill) SELECT device_id,message_id,revision,?2,expires,1 FROM rcs_backfill WHERE device_id=?1 AND expires>?3 AND revision>(SELECT ack_revision FROM v1_devices WHERE id=?1)",params![device.id,sid,now])?;
        rt.metrics
            .backfills
            .fetch_add(moved as u64, Ordering::Relaxed);
    }
    tx.execute("UPDATE rcs_direction SET issued_revision=max(issued_revision,coalesce((SELECT revision FROM rcs_delivery WHERE device_id=?1),0)) WHERE device_id=?1",[&device.id])?;
    tx.commit()?;
    rt.metrics.reports.fetch_add(1, Ordering::Relaxed);
    if source == "WS" {
        rt.metrics.ws.fetch_add(1, Ordering::Relaxed);
    } else {
        rt.metrics.http.fetch_add(1, Ordering::Relaxed);
    }
    if update.is_none() {
        rt.metrics.revoked.fetch_add(1, Ordering::Relaxed);
    }
    tracing::debug!(event="sync.direction.accepted",device_id=%device.id,version,source,operation=op,paused);
    Ok(Acknowledged {
        device_id: device.id.clone(),
        session_id: sid.clone(),
        version,
        server_time: now,
        expires_at: update.map(|_| now + TTL),
        revoked: update.is_none(),
    })
}
pub fn session(
    db: &Connection,
    rt: &Runtime,
    device: &Device,
    sid: &str,
    push: bool,
    now: i64,
) -> Result<(), Fault> {
    ensure_device(db, device)?;
    expire(db, rt, now)?;
    let old = snapshot(db, &device.id)?;
    if old.session_id.as_deref() != Some(sid) || old.direction.is_none() {
        return Err(Fault::revoked());
    }
    let lease: i64 = db.query_row(
        "SELECT lease_until FROM rcs_direction WHERE device_id=?1",
        [&device.id],
        |r| r.get(0),
    )?;
    if lease <= now && !old.paused {
        db.execute("DELETE FROM rcs_delivery WHERE device_id=?1", [&device.id])?;
        return Err(Fault::new(StatusCode::CONFLICT, "SESSION_OFFLINE"));
    }
    if old.paused || (push && old.direction.as_deref() == Some("RECEIVE_ONLY")) {
        return Err(Fault::new(StatusCode::FORBIDDEN, "DIRECTION_FORBIDDEN"));
    }
    db.execute(
        "UPDATE rcs_direction SET lease_until=?2 WHERE device_id=?1",
        params![device.id, now + LEASE],
    )?;
    touch(db, rt, &device.id, now)?;
    Ok(())
}
pub fn ack_session(
    db: &Connection,
    rt: &Runtime,
    device: &Device,
    sid: &str,
    now: i64,
) -> Result<(), Fault> {
    ensure_device(db, device)?;
    expire(db, rt, now)?;
    let old = snapshot(db, &device.id)?;
    if old.session_id.as_deref() != Some(sid) || old.direction.is_none() {
        return Err(Fault::revoked());
    }
    touch(db, rt, &device.id, now)?;
    Ok(())
}
pub fn sid(headers: &HeaderMap) -> Result<&str, Fault> {
    headers
        .get("x-rcs-sync-session")
        .and_then(|s| s.to_str().ok())
        .filter(|s| valid_id(s))
        .ok_or_else(Fault::revoked)
}
pub fn distribute(
    db: &Connection,
    rt: &Runtime,
    source: &str,
    id: &str,
    revision: i64,
    now: i64,
) -> Result<(), StatusCode> {
    expire(db, rt, now).map_err(db_error)?;
    let minutes = window(db, rt);
    let mut stmt=db.prepare("SELECT d.id,s.session_id,s.direction,s.paused,s.lease_until FROM v1_devices d LEFT JOIN rcs_direction s ON s.device_id=d.id WHERE d.id<>?1").map_err(db_error)?;
    let rows = stmt
        .query_map([source], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<bool>>(3)?.unwrap_or(false),
                r.get::<_, Option<i64>>(4)?.unwrap_or(0),
            ))
        })
        .map_err(db_error)?;
    for row in rows {
        let (device, sid, dir, paused, lease) = row.map_err(db_error)?;
        if paused || dir.as_deref().is_none_or(|d| d == "PUSH_ONLY") {
            rt.metrics.filtered.fetch_add(1, Ordering::Relaxed);
            if dir.is_none() {
                rt.metrics.unknown.fetch_add(1, Ordering::Relaxed);
            }
            tracing::debug!(event="sync.delivery.filtered",device_id=%device,message_id=id,reason=if paused{"paused"}else if dir.is_none(){"unknown"}else{"push_only"});
            continue;
        }
        // A fixed-expiry shadow exists even for online delivery: a disconnect
        // detection gap cannot turn a stale unacknowledged live record into history.
        if minutes > 0 {
            db.execute("INSERT INTO rcs_backfill(device_id,message_id,revision,expires) VALUES(?1,?2,?3,?4) ON CONFLICT(device_id) DO UPDATE SET message_id=excluded.message_id,revision=excluded.revision,expires=excluded.expires",params![device,id,revision,now+i64::from(minutes)*60]).map_err(db_error)?;
        }
        if lease > now
            && let Some(sid) = sid
        {
            db.execute("INSERT INTO rcs_delivery(device_id,message_id,revision,session_id,expires,backfill) VALUES(?1,?2,?3,?4,NULL,0) ON CONFLICT(device_id) DO UPDATE SET message_id=excluded.message_id,revision=excluded.revision,session_id=excluded.session_id,expires=NULL,backfill=0",params![device,id,revision,sid]).map_err(db_error)?;
        }
    }
    db.execute("UPDATE rcs_direction SET issued_revision=max(issued_revision,coalesce((SELECT revision FROM rcs_delivery WHERE rcs_delivery.device_id=rcs_direction.device_id),0))",[]).map_err(db_error)?;
    Ok(())
}
pub fn delivery(db: &Connection, device: &Device) -> rusqlite::Result<Option<(String, i64)>> {
    db.query_row("SELECT q.message_id,q.revision FROM rcs_delivery q JOIN rcs_direction s ON s.device_id=q.device_id JOIN v1_devices d ON d.id=q.device_id WHERE q.device_id=?1 AND s.direction IN('RECEIVE_ONLY','BIDIRECTIONAL') AND s.paused=0 AND q.session_id=s.session_id AND q.revision>d.ack_revision",[&device.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()
}
pub fn issued(db: &Connection, id: &str) -> rusqlite::Result<i64> {
    db.query_row(
        "SELECT issued_revision FROM rcs_direction WHERE device_id=?1",
        [id],
        |r| r.get(0),
    )
}
pub fn acknowledge(
    db: &Connection,
    id: &str,
    message: &str,
    revision: i64,
) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM rcs_delivery WHERE device_id=?1 AND message_id=?2 AND revision=?3",
        params![id, message, revision],
    )?;
    db.execute(
        "DELETE FROM rcs_backfill WHERE device_id=?1 AND message_id=?2 AND revision=?3",
        params![id, message, revision],
    )?;
    Ok(())
}
pub async fn protocol(request: Request<Body>, next: Next) -> Response {
    let path = request.uri().path();
    let management = (path == "/api/devices" && request.method() == axum::http::Method::GET)
        || (request.method() == axum::http::Method::DELETE
            && path
                .strip_prefix("/api/devices/")
                .is_some_and(|p| !p.contains('/')));
    let mut response = if !management
        && request
            .headers()
            .get("x-rcs-protocol")
            .and_then(|v| v.to_str().ok())
            != Some(PROTOCOL)
    {
        (
            StatusCode::UPGRADE_REQUIRED,
            Json(serde_json::json!({"code":"PROTOCOL_UNSUPPORTED","minimumProtocolVersion":3})),
        )
            .into_response()
    } else {
        next.run(request).await
    };
    if response.status() == StatusCode::UNAUTHORIZED {
        response = (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"code":"UNAUTHENTICATED"})),
        )
            .into_response();
    }
    response.headers_mut().insert(
        "x-rcs-protocol",
        axum::http::HeaderValue::from_static(PROTOCOL),
    );
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
pub fn router() -> Router<Shared> {
    Router::new()
        .route(
            "/api/devices/{id}/sync-direction",
            get(read).merge(put(update)).delete(revoke),
        )
        .route("/api/ws", get(upgrade))
        .layer(DefaultBodyLimit::max(4096))
        .layer(middleware::from_fn(protocol))
}
async fn read(
    State(app): State<Shared>,
    device: Device,
    Path(id): Path<String>,
) -> Result<Json<Snapshot>, Fault> {
    ownership(&device, &id)?;
    let db = app.db.lock().await;
    ensure_device(&db, &device)?;
    expire(&db, &app.direction, crate::presence::unix_time())?;
    Ok(Json(snapshot(&db, &id)?))
}
async fn update(
    State(app): State<Shared>,
    device: Device,
    Path(id): Path<String>,
    input: Result<Json<Update>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Acknowledged>, Fault> {
    ownership(&device, &id)?;
    let Json(input) = input.map_err(|_| Fault::invalid())?;
    let result = mutate(
        &mut *app.db.lock().await,
        &app.direction,
        &device,
        Some(&input),
        None,
        "HTTP",
        crate::presence::unix_time(),
    );
    if result.is_err() {
        app.direction.metrics.errors.fetch_add(1, Ordering::Relaxed);
    }
    let ack = result?;
    let _ = app.events.send(());
    Ok(Json(ack))
}
async fn revoke(
    State(app): State<Shared>,
    device: Device,
    Path(id): Path<String>,
    input: Result<Json<Revoke>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Acknowledged>, Fault> {
    ownership(&device, &id)?;
    let Json(input) = input.map_err(|_| Fault::invalid())?;
    let ack = mutate(
        &mut *app.db.lock().await,
        &app.direction,
        &device,
        None,
        Some(&input),
        "HTTP",
        crate::presence::unix_time(),
    )?;
    let _ = app.events.send(());
    Ok(Json(ack))
}
#[derive(Deserialize)]
#[serde(tag = "type")]
enum Frame {
    SyncDirectionUpdate {
        #[serde(flatten)]
        input: Update,
    },
    SyncDirectionRevoke {
        #[serde(flatten)]
        input: Revoke,
    },
}
async fn upgrade(
    State(app): State<Shared>,
    device: Device,
    ws: WebSocketUpgrade,
) -> Result<Response, Fault> {
    let permit = Arc::clone(&app.direction.sockets)
        .try_acquire_owned()
        .map_err(|_| Fault::rate())?;
    Ok(ws
        .max_message_size(4096)
        .max_frame_size(4096)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            socket_loop(socket, app, device).await;
        }))
}
async fn socket_loop(mut socket: WebSocket, app: Shared, device: Device) {
    let mut bound: Option<String> = None;
    let mut pong = Instant::now();
    let mut timer = tokio::time::interval(Duration::from_secs(15));
    timer.tick().await;
    loop {
        tokio::select! {
            _=timer.tick()=>{
                if pong.elapsed()>Duration::from_secs(45){break;}
                if let Some(sid)=&bound {let db=app.db.lock().await;if session(&db,&app.direction,&device,sid,false,crate::presence::unix_time()).is_err(){break;}}
                if socket.send(Message::Ping(vec![].into())).await.is_err(){break;}
            }
            msg=socket.recv()=>{
                let Some(Ok(msg))=msg else{break;};
                let text=match msg {Message::Text(t)=>t,Message::Pong(_)=>{pong=Instant::now();continue;},Message::Ping(_)=>continue,Message::Close(_)=>break,_=>break};
                let frame=serde_json::from_str::<Frame>(&text);
                let result=match frame {
                    Ok(Frame::SyncDirectionUpdate{input})=>{
                        if bound.as_ref().is_some_and(|sid|sid!=&input.session_id) || (bound.is_none()&&!input.connect) {Err(Fault::revoked())}
                        else {let result=mutate(&mut *app.db.lock().await,&app.direction,&device,Some(&input),None,"WS",crate::presence::unix_time());if result.is_ok(){bound=Some(input.session_id);}result}
                    }
                    Ok(Frame::SyncDirectionRevoke{input})=>{
                        if bound.as_deref()!=Some(&input.session_id){Err(Fault::revoked())}else{mutate(&mut *app.db.lock().await,&app.direction,&device,None,Some(&input),"WS",crate::presence::unix_time())}
                    }
                    Err(_)=>Err(Fault::invalid())
                };
                let value=match result {Ok(ack)=>{let mut v=serde_json::to_value(ack).unwrap_or(serde_json::Value::Null);v["type"]=serde_json::json!("SyncDirectionAck");let _=app.events.send(());v},Err(f)=>{app.direction.metrics.errors.fetch_add(1,Ordering::Relaxed);serde_json::json!({"type":"SyncDirectionError","code":f.code,"version":f.version})}};
                if socket.send(Message::Text(value.to_string().into())).await.is_err(){break;}
            }
        }
    }
    // Release only this session's lease; an old close must not fence a new one.
    if let Some(sid) = bound {
        let db = app.db.lock().await;
        let _ = db.execute(
            "UPDATE rcs_direction SET lease_until=0 WHERE device_id=?1 AND session_id=?2 AND source='WS'",
            params![device.id, sid],
        );
    }
}

#[cfg(test)]
mod tests;
