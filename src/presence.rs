//! Presence is recent authenticated activity, not proof a device is reachable.
use axum::http::StatusCode;
use rusqlite::{Connection, params};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy)]
struct Seen {
    at: Instant,
    persisted: Instant,
    unix: i64,
}
#[derive(Default)]
pub struct Presence {
    seen: Mutex<HashMap<String, Seen>>,
}
pub fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
impl Presence {
    pub fn record(&self, db: &Connection, id: &str) -> Result<(), StatusCode> {
        self.record_at(db, id, Instant::now(), unix_time())
    }
    fn record_at(
        &self,
        db: &Connection,
        id: &str,
        now: Instant,
        unix: i64,
    ) -> Result<(), StatusCode> {
        let mut seen = self
            .seen
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let persist = seen
            .get(id)
            .is_none_or(|s| now.duration_since(s.persisted) >= Duration::from_secs(60));
        if persist {
            db.execute(
                "UPDATE v1_devices SET last_seen=?1 WHERE id=?2",
                params![unix, id],
            )
            .map_err(crate::db_error)?;
        }
        if seen.len() >= 64
            && !seen.contains_key(id)
            && let Some(oldest) = seen
                .iter()
                .min_by_key(|(_, s)| s.at)
                .map(|(id, _)| id.clone())
        {
            seen.remove(&oldest);
        }
        let persisted = if persist {
            now
        } else {
            seen.get(id).map_or(now, |s| s.persisted)
        };
        seen.insert(
            id.to_owned(),
            Seen {
                at: now,
                persisted,
                unix,
            },
        );
        Ok(())
    }
    pub fn status(
        &self,
        id: &str,
        persisted: Option<i64>,
    ) -> Result<(bool, Option<i64>), StatusCode> {
        self.status_at(id, persisted, Instant::now())
    }
    fn status_at(
        &self,
        id: &str,
        persisted: Option<i64>,
        now: Instant,
    ) -> Result<(bool, Option<i64>), StatusCode> {
        let seen = self
            .seen
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(seen.get(id).map_or((false, persisted), |s| {
            (
                now.duration_since(s.at) < Duration::from_secs(60),
                Some(s.unix),
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presence_expires_without_timers_and_writes_are_throttled() {
        let db = Connection::open_in_memory().unwrap();
        crate::api::init(&db).unwrap();
        db.execute(
            "INSERT INTO v1_devices(id,token_hash,ack_revision) VALUES('fixture',x'01',0)",
            [],
        )
        .unwrap();
        db.execute_batch("CREATE TABLE writes(n INTEGER); INSERT INTO writes VALUES(0); CREATE TRIGGER count_seen AFTER UPDATE OF last_seen ON v1_devices BEGIN UPDATE writes SET n=n+1; END;").unwrap();
        let presence = Presence::default();
        let start = Instant::now();
        presence.record_at(&db, "fixture", start, 100).unwrap();
        presence
            .record_at(&db, "fixture", start + Duration::from_secs(20), 120)
            .unwrap();
        assert_eq!(
            db.query_row("SELECT n FROM writes", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            presence
                .status_at("fixture", Some(100), start + Duration::from_secs(79))
                .unwrap(),
            (true, Some(120))
        );
        assert_eq!(
            presence
                .status_at("fixture", Some(100), start + Duration::from_secs(80))
                .unwrap(),
            (false, Some(120))
        );
        presence
            .record_at(&db, "fixture", start + Duration::from_secs(60), 160)
            .unwrap();
        assert_eq!(
            db.query_row("SELECT n FROM writes", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            Presence::default()
                .status_at("fixture", Some(160), start)
                .unwrap(),
            (false, Some(160)),
            "restart must not invent online status from a persisted timestamp"
        );
    }
}
