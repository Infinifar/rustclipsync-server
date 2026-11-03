use super::*;

#[test]
fn clearing_metrics_preserves_runtime_controls_and_counts_new_events() {
    let rt = Runtime::default();
    let m = &rt.metrics;
    let counters = [
        &m.reports,
        &m.errors,
        &m.ws,
        &m.http,
        &m.filtered,
        &m.unknown,
        &m.revoked,
        &m.expired,
        &m.backfills,
        &m.alerts,
    ];
    for counter in counters {
        counter.store(12, Ordering::Relaxed);
    }
    rt.warned.store(true, Ordering::Relaxed);
    rt.rates
        .lock()
        .unwrap()
        .insert("fixture".into(), (Instant::now(), 120));
    let permit = rt.sockets.try_acquire().unwrap();
    reset_statistics(&rt);
    for counter in counters {
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
    assert!(
        rt.warned.load(Ordering::Relaxed),
        "reset must not re-arm configuration alerts"
    );
    assert_eq!(
        rt.rates.lock().unwrap()["fixture"].1,
        120,
        "reset must not clear abuse limits"
    );
    assert_eq!(
        rt.sockets.available_permits(),
        31,
        "reset must not release live sockets"
    );
    drop(permit);
    m.reports.fetch_add(1, Ordering::Relaxed);
    assert_eq!(m.reports.load(Ordering::Relaxed), 1);
}
use axum::{
    body::to_bytes,
    http::{Method, Request},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, broadcast};
use tower::ServiceExt;
fn fixture() -> (Shared, Router) {
    let db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let (events, _) = broadcast::channel(16);
    let app = Arc::new(crate::App {
        db: AsyncMutex::new(db),
        user: "fixture".into(),
        password: "fixture-password".into(),
        events,
        waiters: Arc::new(Semaphore::new(8)),
        uploads: Arc::new(Semaphore::new(2)),
        auth: crate::auth::AuthState::default(),
        web: crate::admin::WebState::default(),
        presence: crate::presence::Presence::default(),
        direction: Runtime::default(),
    });
    (Arc::clone(&app), crate::router(app))
}
fn identity(db: &Connection) -> Device {
    let id = uuid::Uuid::new_v4().to_string();
    let hash = Sha256::digest("ab".repeat(32).as_bytes()).to_vec();
    db.execute(
        "INSERT INTO v1_devices(id,token_hash,ack_revision) VALUES(?1,?2,0)",
        params![id, hash],
    )
    .unwrap();
    Device {
        id,
        token_hash: hash,
    }
}
fn update_request(version: i64) -> Update {
    Update {
        session_id: uuid::Uuid::new_v4().to_string(),
        version,
        direction: SyncDirection::Bidirectional,
        paused: false,
        connect: true,
    }
}
async fn send(
    app: &Router,
    device: &Device,
    sid: Option<&str>,
    method: Method,
    path: &str,
    body: Value,
) -> Response {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-rcs-protocol", PROTOCOL)
        .header(
            "authorization",
            format!("Bearer {}:{}", device.id, "ab".repeat(32)),
        )
        .header("content-type", "application/json");
    if let Some(sid) = sid {
        req = req.header("x-rcs-sync-session", sid);
    }
    app.clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}
async fn json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
fn control(
    db: &mut Connection,
    rt: &Runtime,
    device: &Device,
    dir: SyncDirection,
    now: i64,
) -> Update {
    let mut req = update_request(snapshot(db, &device.id).unwrap().version + 1);
    req.direction = dir;
    mutate(db, rt, device, Some(&req), None, "HTTP", now).unwrap();
    req
}
fn message(db: &Connection, source: &Device, n: i64, now: i64) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    db.execute("INSERT INTO v1_messages(revision,message_id,device_id,base_revision,body_hash,body,created,sensitive) VALUES(?1,?2,?3,?4,x'01',x'66697874757265',?5,0)",params![n,id,source.id,n-1,now]).unwrap();
    db.execute("UPDATE v1_meta SET revision=?1", [n]).unwrap();
    id
}
#[test]
fn session_cas_idempotency_pause_revoke_and_ttl_are_fenced() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let rt = Runtime::default();
    let d = identity(&db);
    let mut a = control(&mut db, &rt, &d, SyncDirection::Bidirectional, 100);
    assert_eq!(
        mutate(&mut db, &rt, &d, Some(&a), None, "HTTP", 101)
            .unwrap()
            .version,
        1
    );
    a.paused = true;
    assert_eq!(
        mutate(&mut db, &rt, &d, Some(&a), None, "HTTP", 102)
            .unwrap_err()
            .code,
        "VERSION_STALE"
    );
    a.version = 2;
    a.connect = false;
    mutate(&mut db, &rt, &d, Some(&a), None, "HTTP", 102).unwrap();
    assert_eq!(
        session(&db, &rt, &d, &a.session_id, false, 103)
            .unwrap_err()
            .code,
        "DIRECTION_FORBIDDEN"
    );
    assert!(ack_session(&db, &rt, &d, &a.session_id, 103).is_ok());
    let r = Revoke {
        session_id: a.session_id.clone(),
        version: 3,
    };
    mutate(&mut db, &rt, &d, None, Some(&r), "HTTP", 104).unwrap();
    assert!(
        mutate(&mut db, &rt, &d, None, Some(&r), "HTTP", 105)
            .unwrap()
            .revoked
    );
    a.version = 4;
    a.paused = false;
    assert_eq!(
        mutate(&mut db, &rt, &d, Some(&a), None, "HTTP", 105)
            .unwrap_err()
            .code,
        "SESSION_REVOKED"
    );
    let b = control(&mut db, &rt, &d, SyncDirection::ReceiveOnly, 106);
    assert_ne!(b.session_id, a.session_id);
    assert_eq!(
        session(&db, &rt, &d, &a.session_id, false, 107)
            .unwrap_err()
            .code,
        "SESSION_REVOKED"
    );
    touch(&db, &rt, &d.id, 107).unwrap();
    assert_eq!(
        snapshot(&db, &d.id).unwrap().expires_at,
        Some(107 + TTL),
        "each valid activity refreshes the complete 30-day TTL"
    );
    touch(&db, &rt, &d.id, 106 + TTL + 1).unwrap();
    assert!(
        snapshot(&db, &d.id).unwrap().revoked,
        "expired state must not revive through activity"
    );
    assert_eq!(snapshot(&db, &d.id).unwrap().version, 4);
}
#[test]
fn per_device_backfill_is_latest_fixed_expiry_and_zero_cannot_revive_it() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let rt = Runtime::default();
    let source = identity(&db);
    let receiver = identity(&db);
    let pusher = identity(&db);
    let unknown = identity(&db);
    let recv = control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 100);
    control(&mut db, &rt, &pusher, SyncDirection::PushOnly, 100);
    let first = message(&db, &source, 1, 101);
    distribute(&db, &rt, &source.id, &first, 1, 101).unwrap();
    assert_eq!(delivery(&db, &receiver).unwrap(), Some((first.clone(), 1)));
    assert!(delivery(&db, &pusher).unwrap().is_none());
    assert!(delivery(&db, &unknown).unwrap().is_none());
    acknowledge(&db, &receiver.id, &first, 1).unwrap();
    assert!(delivery(&db, &receiver).unwrap().is_none());
    db.execute(
        "UPDATE rcs_direction SET lease_until=0 WHERE device_id=?1",
        [&receiver.id],
    )
    .unwrap();
    let second = message(&db, &source, 2, 110);
    distribute(&db, &rt, &source.id, &second, 2, 110).unwrap();
    let third = message(&db, &source, 3, 120);
    distribute(&db, &rt, &source.id, &third, 3, 120).unwrap();
    assert!(delivery(&db, &receiver).unwrap().is_none());
    assert_eq!(
        db.query_row(
            "SELECT message_id,expires FROM rcs_backfill WHERE device_id=?1",
            [&receiver.id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        )
        .unwrap(),
        (third.clone(), 420)
    );
    set_window(&mut db, &rt, "fixture-admin", 1).unwrap(); // does not change the stored 420 expiry
    let new = control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 200);
    assert_eq!(delivery(&db, &receiver).unwrap(), Some((third.clone(), 3)));
    assert_ne!(new.session_id, recv.session_id);
    acknowledge(&db, &receiver.id, &second, 2).unwrap();
    assert_eq!(
        delivery(&db, &receiver).unwrap(),
        Some((third, 3)),
        "old ack must not erase a newer candidate"
    );
    set_window(&mut db, &rt, "fixture-admin", 0).unwrap();
    set_window(&mut db, &rt, "fixture-admin", 5).unwrap();
    control(&mut db, &rt, &receiver, SyncDirection::Bidirectional, 201);
    assert!(
        delivery(&db, &receiver).unwrap().is_none(),
        "disabled candidates must never return"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM rcs_settings_audit", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
}
#[test]
fn configuration_failure_is_zero_and_device_deletion_cascades_all_state() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let rt = Runtime::default();
    let d = identity(&db);
    let source = identity(&db);
    control(&mut db, &rt, &d, SyncDirection::Bidirectional, 1);
    assert_eq!(window(&db, &rt), 5);
    assert_eq!(
        set_window(&mut db, &rt, "fixture", -1),
        Err(StatusCode::BAD_REQUEST)
    );
    assert_eq!(
        set_window(&mut db, &rt, "fixture", 1441),
        Err(StatusCode::BAD_REQUEST)
    );
    for invalid in [
        "UPDATE rcs_settings SET value=1441",
        "DELETE FROM rcs_settings",
    ] {
        db.execute(invalid, []).unwrap();
        assert_eq!(window(&db, &rt), 0);
        crate::api::init(&db).unwrap();
        assert_eq!(
            window(&db, &rt),
            0,
            "restart must not silently re-enable corrupted/missing settings"
        );
    }
    set_window(&mut db, &rt, "fixture", 5).unwrap();
    let id = message(&db, &source, 1, 2);
    distribute(&db, &rt, &source.id, &id, 1, 2).unwrap();
    db.execute("DELETE FROM v1_devices WHERE id=?1", [&d.id])
        .unwrap();
    for table in ["rcs_direction", "rcs_delivery", "rcs_backfill"] {
        assert_eq!(
            db.query_row(
                &format!("SELECT count(*) FROM {table} WHERE device_id=?1"),
                [&d.id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "{table}"
        );
    }
}
#[tokio::test]
async fn http_protocol_gate_ownership_and_content_cannot_bypass_filter() {
    let (app, router) = fixture();
    let (source, receive, push) = {
        let db = app.db.lock().await;
        (identity(&db), identity(&db), identity(&db))
    };
    let now = crate::presence::unix_time();
    let (s, r, p) = {
        let mut db = app.db.lock().await;
        (
            control(
                &mut db,
                &app.direction,
                &source,
                SyncDirection::Bidirectional,
                now,
            ),
            control(
                &mut db,
                &app.direction,
                &receive,
                SyncDirection::ReceiveOnly,
                now,
            ),
            control(&mut db, &app.direction, &push, SyncDirection::PushOnly, now),
        )
    };
    let no_version = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_version.status(), StatusCode::UPGRADE_REQUIRED);
    assert_eq!(json(no_version).await["code"], "PROTOCOL_UNSUPPORTED");
    let mismatch = send(
        &router,
        &source,
        None,
        Method::PUT,
        &format!("/api/devices/{}/sync-direction", receive.id),
        serde_json::to_value(&s).unwrap(),
    )
    .await;
    assert_eq!(mismatch.status(), StatusCode::FORBIDDEN);
    let id = {
        let db = app.db.lock().await;
        let id = message(&db, &source, 1, now);
        distribute(&db, &app.direction, &source.id, &id, 1, now).unwrap();
        id
    };
    let state = json(
        send(
            &router,
            &push,
            Some(&p.session_id),
            Method::GET,
            "/api/state",
            Value::Null,
        )
        .await,
    )
    .await;
    assert_eq!(state["revision"], 1);
    assert!(state["messageId"].is_null());
    assert_eq!(
        send(
            &router,
            &push,
            Some(&p.session_id),
            Method::GET,
            &format!("/api/messages/{id}/content"),
            Value::Null
        )
        .await
        .status(),
        StatusCode::GONE
    );
    assert_eq!(
        send(
            &router,
            &receive,
            Some(&r.session_id),
            Method::GET,
            &format!("/api/messages/{id}/content"),
            Value::Null
        )
        .await
        .status(),
        StatusCode::OK
    );
    let next = control(
        &mut *app.db.lock().await,
        &app.direction,
        &receive,
        SyncDirection::ReceiveOnly,
        now,
    );
    let stale = send(
        &router,
        &receive,
        Some(&r.session_id),
        Method::GET,
        "/api/state",
        Value::Null,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json(stale).await["code"], "SESSION_REVOKED");
    assert_eq!(
        send(
            &router,
            &receive,
            Some(&next.session_id),
            Method::GET,
            "/api/state",
            Value::Null
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_and_http_share_cas_and_old_close_cannot_disable_new_session() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{Message as WsMessage, client::IntoClientRequest},
    };
    let (app, router) = fixture();
    let device = identity(&*app.db.lock().await);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let mut request = format!("ws://{address}/api/ws")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("x-rcs-protocol", "3".parse().unwrap());
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}:{}", device.id, "ab".repeat(32))
            .parse()
            .unwrap(),
    );
    let (mut socket, response) = connect_async(request).await.unwrap();
    assert_eq!(response.headers()["x-rcs-protocol"], "3");
    let input = update_request(1);
    let mut frame = serde_json::to_value(&input).unwrap();
    frame["type"] = serde_json::json!("SyncDirectionUpdate");
    socket
        .send(WsMessage::Text(frame.to_string().into()))
        .await
        .unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    let ack: Value = serde_json::from_str(&ack).unwrap();
    assert_eq!(ack["type"], "SyncDirectionAck");
    assert_eq!(ack["version"], 1);
    let now = crate::presence::unix_time();
    {
        let mut db = app.db.lock().await;
        assert_eq!(
            mutate(
                &mut db,
                &app.direction,
                &device,
                Some(&input),
                None,
                "HTTP",
                now
            )
            .unwrap()
            .version,
            1
        );
    }
    let replacement = control(
        &mut *app.db.lock().await,
        &app.direction,
        &device,
        SyncDirection::ReceiveOnly,
        now,
    );
    let mut stale = input.clone();
    stale.version = 3;
    stale.connect = false;
    let mut frame = serde_json::to_value(&stale).unwrap();
    frame["type"] = serde_json::json!("SyncDirectionUpdate");
    socket
        .send(WsMessage::Text(frame.to_string().into()))
        .await
        .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&error).unwrap()["code"],
        "SESSION_REVOKED"
    );
    socket.close(None).await.unwrap();
    tokio::task::yield_now().await;
    assert!(
        session(
            &*app.db.lock().await,
            &app.direction,
            &device,
            &replacement.session_id,
            false,
            now
        )
        .is_ok()
    );
    task.abort();
}

#[test]
fn offline_lease_requires_fresh_report_and_does_not_leak_stale_live_content() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let rt = Runtime::default();
    let source = identity(&db);
    let receiver = identity(&db);
    let first = control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 100);
    let id = message(&db, &source, 1, 101);
    distribute(&db, &rt, &source.id, &id, 1, 101).unwrap();
    assert_eq!(
        session(&db, &rt, &receiver, &first.session_id, false, 131)
            .unwrap_err()
            .code,
        "SESSION_OFFLINE"
    );
    assert!(delivery(&db, &receiver).unwrap().is_none());
    let new = control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 132);
    assert_ne!(new.session_id, first.session_id);
    assert_eq!(delivery(&db, &receiver).unwrap(), Some((id, 1)));
    set_window(&mut db, &rt, "fixture", 0).unwrap();
    let second = message(&db, &source, 2, 133);
    distribute(&db, &rt, &source.id, &second, 2, 133).unwrap();
    assert!(
        delivery(&db, &receiver).unwrap().is_some(),
        "window zero still permits live delivery"
    );
    control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 134);
    assert!(
        delivery(&db, &receiver).unwrap().is_none(),
        "window zero must not replay old live delivery across sessions"
    );
}

#[test]
fn lost_ws_ack_then_exact_http_fallback_preserves_backfill() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let rt = Runtime::default();
    let source = identity(&db);
    let receiver = identity(&db);
    control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 100);
    db.execute(
        "UPDATE rcs_direction SET lease_until=0 WHERE device_id=?1",
        [&receiver.id],
    )
    .unwrap();
    let id = message(&db, &source, 1, 101);
    distribute(&db, &rt, &source.id, &id, 1, 101).unwrap();
    let request = control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 102);
    assert_eq!(delivery(&db, &receiver).unwrap(), Some((id.clone(), 1)));
    // The server accepted WS connect but its ACK was lost; the socket closes.
    db.execute(
        "UPDATE rcs_direction SET lease_until=0 WHERE device_id=?1",
        [&receiver.id],
    )
    .unwrap();
    mutate(&mut db, &rt, &receiver, Some(&request), None, "HTTP", 103).unwrap();
    assert_eq!(
        delivery(&db, &receiver).unwrap(),
        Some((id, 1)),
        "exact HTTP fallback must restore a candidate erased by WS-close lease cleanup"
    );
    assert_eq!(
        snapshot(&db, &receiver.id).unwrap().version,
        2,
        "fallback must not silently create another session"
    );
}

#[test]
fn late_ws_close_cannot_clear_a_same_session_http_fallback_lease() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let rt = Runtime::default();
    let device = identity(&db);
    let req = update_request(1);
    mutate(&mut db, &rt, &device, Some(&req), None, "WS", 100).unwrap();
    mutate(&mut db, &rt, &device, Some(&req), None, "HTTP", 101).unwrap();
    db.execute("UPDATE rcs_direction SET lease_until=0 WHERE device_id=?1 AND session_id=?2 AND source='WS'",params![device.id,req.session_id]).unwrap();
    assert!(
        session(&db, &rt, &device, &req.session_id, false, 102).is_ok(),
        "old WS close must preserve a confirmed HTTP fallback lease"
    );
}

#[tokio::test]
async fn already_offered_ack_survives_a_newer_delivery_and_pause() {
    let (app, router) = fixture();
    let (source, receiver) = {
        let db = app.db.lock().await;
        (identity(&db), identity(&db))
    };
    let now = crate::presence::unix_time();
    let mut mode = control(
        &mut *app.db.lock().await,
        &app.direction,
        &receiver,
        SyncDirection::ReceiveOnly,
        now,
    );
    let (epoch, first) = {
        let db = app.db.lock().await;
        let epoch: String = db
            .query_row("SELECT epoch FROM v1_meta", [], |r| r.get(0))
            .unwrap();
        let a = message(&db, &source, 1, now);
        distribute(&db, &app.direction, &source.id, &a, 1, now).unwrap();
        (epoch, a)
    };
    assert_eq!(
        send(
            &router,
            &receiver,
            Some(&mode.session_id),
            Method::GET,
            &format!("/api/messages/{first}/content"),
            Value::Null
        )
        .await
        .status(),
        StatusCode::OK
    );
    let second = {
        let db = app.db.lock().await;
        let b = message(&db, &source, 2, now);
        distribute(&db, &app.direction, &source.id, &b, 2, now).unwrap();
        b
    };
    let ack = serde_json::json!({"epoch":epoch,"revision":1,"messageId":first,"result":"applied"});
    assert_eq!(
        send(
            &router,
            &receiver,
            Some(&mode.session_id),
            Method::POST,
            "/api/ack",
            ack
        )
        .await
        .status(),
        StatusCode::OK,
        "a newer candidate must not invalidate an already-issued delivery acknowledgement"
    );
    assert_eq!(
        send(
            &router,
            &receiver,
            Some(&mode.session_id),
            Method::GET,
            &format!("/api/messages/{second}/content"),
            Value::Null
        )
        .await
        .status(),
        StatusCode::OK
    );
    mode.version += 1;
    mode.connect = false;
    mode.paused = true;
    mutate(
        &mut *app.db.lock().await,
        &app.direction,
        &receiver,
        Some(&mode),
        None,
        "HTTP",
        now,
    )
    .unwrap();
    let ack = serde_json::json!({"epoch":epoch,"revision":2,"messageId":second,"result":"applied"});
    assert_eq!(
        send(
            &router,
            &receiver,
            Some(&mode.session_id),
            Method::POST,
            "/api/ack",
            ack
        )
        .await
        .status(),
        StatusCode::OK,
        "pause must allow draining already-issued ACKs without restoring content access"
    );
}
#[test]
fn promoted_backfill_keeps_its_deadline_and_zero_revokes_only_backfill() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let rt = Runtime::default();
    let source = identity(&db);
    let receiver = identity(&db);
    set_window(&mut db, &rt, "fixture", 1).unwrap();
    control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 100);
    db.execute("UPDATE rcs_direction SET lease_until=0", [])
        .unwrap();
    let id = message(&db, &source, 1, 101);
    distribute(&db, &rt, &source.id, &id, 1, 101).unwrap();
    let mode = control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 150);
    assert!(delivery(&db, &receiver).unwrap().is_some());
    session(&db, &rt, &receiver, &mode.session_id, false, 162).unwrap();
    assert!(
        delivery(&db, &receiver).unwrap().is_none(),
        "reconnect must not strip an offline candidate's original deadline"
    );
    db.execute("UPDATE rcs_direction SET lease_until=0", [])
        .unwrap();
    let id = message(&db, &source, 2, 163);
    distribute(&db, &rt, &source.id, &id, 2, 163).unwrap();
    control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 164);
    set_window(&mut db, &rt, "fixture", 0).unwrap();
    assert!(
        delivery(&db, &receiver).unwrap().is_none(),
        "zero must invalidate promoted but unacknowledged offline backfill"
    );
}

#[test]
fn exact_retry_preserves_existing_live_delivery_kind() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let rt = Runtime::default();
    let source = identity(&db);
    let receiver = identity(&db);
    let mut request = control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 100);
    let id = message(&db, &source, 1, 101);
    distribute(&db, &rt, &source.id, &id, 1, 101).unwrap();
    request.version = 2;
    request.connect = false;
    mutate(&mut db, &rt, &receiver, Some(&request), None, "WS", 102).unwrap();
    mutate(&mut db, &rt, &receiver, Some(&request), None, "HTTP", 103).unwrap();
    set_window(&mut db, &rt, "fixture", 0).unwrap();
    assert_eq!(
        delivery(&db, &receiver).unwrap(),
        Some((id, 1)),
        "idempotent retry must not relabel live data as expiring offline backfill"
    );
}

#[test]
fn process_restart_discards_online_lease_but_preserves_fixed_ttl_backfill() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let rt = Runtime::default();
    let source = identity(&db);
    let receiver = identity(&db);
    let request = control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 100);
    let id = message(&db, &source, 1, 101);
    distribute(&db, &rt, &source.id, &id, 1, 101).unwrap();
    restart(&mut db).unwrap();
    assert!(delivery(&db, &receiver).unwrap().is_none());
    assert_eq!(
        session(&db, &rt, &receiver, &request.session_id, false, 102)
            .unwrap_err()
            .code,
        "SESSION_OFFLINE"
    );
    control(&mut db, &rt, &receiver, SyncDirection::ReceiveOnly, 103);
    assert_eq!(delivery(&db, &receiver).unwrap(), Some((id, 1)));
}
