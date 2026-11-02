use super::*;
use axum::{
    body::to_bytes,
    http::{Method, Request},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, Semaphore, broadcast};
use tower::ServiceExt;

const PASSWORD: &str = "fixture-admin-password";
const DEVICE_PASSWORD: &str = "fixture-device-password";
fn app() -> (Shared, Router) {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let (events, _) = broadcast::channel(16);
    let app = Arc::new(crate::App {
        db: AsyncMutex::new(db),
        user: "legacy".into(),
        password: "fixture-legacy-password".into(),
        events,
        waiters: Arc::new(Semaphore::new(8)),
        uploads: Arc::new(Semaphore::new(2)),
        auth: auth::AuthState::new(Some(
            auth::Credentials::new("administrator".into(), PASSWORD.into()).unwrap(),
        )),
        web: WebState::enabled("https://admin.test").unwrap(),
        presence: crate::presence::Presence::default(),
        direction: crate::direction::Runtime::default(),
    });
    (Arc::clone(&app), crate::router(app))
}
async fn send(
    app: &Router,
    method: Method,
    path: &str,
    extra: &[(&str, &str)],
    body: Value,
) -> Response {
    let mut request = Request::builder()
        .header("x-rcs-protocol", "3")
        .method(method)
        .uri(path)
        .header(header::HOST, "admin.test")
        .header(header::CONTENT_TYPE, "application/json");
    for (key, value) in extra {
        request = request.header(*key, *value);
        if *key == "authorization"
            && let Some((id, _)) = value
                .strip_prefix("Bearer ")
                .and_then(|s| s.split_once(':'))
        {
            request = request.header("x-rcs-sync-session", id);
        }
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}
async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
fn basic(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}
async fn login(app: &Router) -> (String, String) {
    let response = send(
        app,
        Method::POST,
        "/api/admin/login",
        &[("origin", "https://admin.test"), ("x-rcs-admin", "1")],
        json!({"username":"administrator","password":PASSWORD}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cookie.contains("Secure; HttpOnly; SameSite=Strict; Path=/"));
    assert!(!cookie.contains("Domain="));
    let cookie = cookie.split(';').next().unwrap().to_owned();
    let value = body(response).await;
    (cookie, value["csrfToken"].as_str().unwrap().to_owned())
}
async fn create(app: &Router, cookie: &str, csrf: &str, name: &str) -> String {
    let response = send(
        app,
        Method::POST,
        "/api/admin/accounts",
        &[
            ("origin", "https://admin.test"),
            ("cookie", cookie),
            ("x-csrf-token", csrf),
        ],
        json!({"username":name,"password":DEVICE_PASSWORD}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let value = body(response).await;
    assert!(value.get("password").is_none());
    assert!(value.get("passwordHash").is_none());
    value["id"].as_str().unwrap().to_owned()
}
async fn register(app: &Router, user: &str, password: &str) -> (String, String, Value) {
    let id = uuid::Uuid::new_v4().to_string();
    let token = "ab".repeat(32);
    let response = send(
        app,
        Method::POST,
        "/api/devices",
        &[("authorization", &basic(user, password))],
        json!({"deviceId":id,"deviceToken":token}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let control=send(app,Method::PUT,&format!("/api/devices/{id}/sync-direction"),&[("authorization",&format!("Bearer {id}:{token}"))],json!({"sessionId":id,"version":1,"direction":"BIDIRECTIONAL","paused":false,"connect":true})).await;
    assert_eq!(control.status(), StatusCode::OK);
    (
        id.clone(),
        format!("Bearer {id}:{token}"),
        body(response).await,
    )
}

#[test]
fn web_origin_rejects_non_https_or_non_origin_urls() {
    for origin in [
        "http://admin.test",
        "https://user:pass@admin.test",
        "https://admin.test/admin",
        "https://admin.test?x=1",
        "https://admin.test/#x",
    ] {
        assert!(WebState::enabled(origin).is_err(), "{origin}");
    }
    assert_eq!(
        WebState::enabled("https://admin.test:443/")
            .unwrap()
            .origin
            .as_deref(),
        Some("https://admin.test")
    );
}

#[tokio::test]
async fn default_web_is_disabled_and_existing_credentials_still_sync() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    crate::api::init(&db).unwrap();
    let (events, _) = broadcast::channel(16);
    let state = Arc::new(crate::App {
        db: AsyncMutex::new(db),
        user: "legacy".into(),
        password: "fixture-legacy-password".into(),
        events,
        waiters: Arc::new(Semaphore::new(8)),
        uploads: Arc::new(Semaphore::new(2)),
        auth: auth::AuthState::default(),
        web: WebState::default(),
        presence: crate::presence::Presence::default(),
        direction: crate::direction::Runtime::default(),
    });
    let app = crate::router(state);
    assert_eq!(
        send(&app, Method::GET, "/admin/", &[], Value::Null)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            Method::POST,
            "/api/admin/login",
            &[("origin", "https://admin.test"), ("x-rcs-admin", "1")],
            json!({"username":"legacy","password":"fixture-legacy-password"})
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let (_, token, _) = register(&app, "legacy", "fixture-legacy-password").await;
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/state",
            &[("authorization", &token)],
            Value::Null
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn login_requires_independent_admin_and_protects_origin_and_headers() {
    let (_, app) = app();
    for user in ["legacy", "missing"] {
        assert_eq!(
            send(
                &app,
                Method::POST,
                "/api/admin/login",
                &[("origin", "https://admin.test"), ("x-rcs-admin", "1")],
                json!({"username":user,"password":"fixture-legacy-password"})
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for headers in [
        vec![],
        vec![("origin", "https://evil.test"), ("x-rcs-admin", "1")],
        vec![("origin", "https://admin.test")],
        vec![
            ("origin", "https://evil.test"),
            ("x-rcs-admin", "1"),
            ("x-forwarded-host", "admin.test"),
        ],
    ] {
        assert_eq!(
            send(
                &app,
                Method::POST,
                "/api/admin/login",
                &headers,
                json!({"username":"administrator","password":PASSWORD})
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    let request = Request::builder()
        .header("x-rcs-protocol", "3")
        .method(Method::POST)
        .uri("/api/admin/login")
        .header(header::HOST, "evil.test")
        .header(header::ORIGIN, "https://admin.test")
        .header("x-rcs-admin", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"username":"administrator","password":PASSWORD}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let request = Request::builder()
        .header("x-rcs-protocol", "3")
        .method(Method::POST)
        .uri("/api/admin/login")
        .header(header::HOST, "admin.test")
        .header(header::ORIGIN, "https://admin.test")
        .header("x-rcs-admin", "1")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(
            "username=administrator&password=fixture-admin-password",
        ))
        .unwrap();
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn sessions_enforce_csrf_logout_expiry_and_cookie_rotation() {
    let (shared, app) = app();
    let (cookie, csrf) = login(&app).await;
    for headers in [
        vec![
            ("cookie", cookie.as_str()),
            ("origin", "https://admin.test"),
        ],
        vec![
            ("cookie", cookie.as_str()),
            ("origin", "https://evil.test"),
            ("x-csrf-token", csrf.as_str()),
        ],
        vec![
            ("cookie", cookie.as_str()),
            ("origin", "https://admin.test"),
            ("x-csrf-token", "wrong"),
        ],
    ] {
        assert_eq!(
            send(
                &app,
                Method::POST,
                "/api/admin/logout",
                &headers,
                Value::Null
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/devices",
            &[("cookie", &cookie)],
            Value::Null
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
        "browser cookie must not authorize native API"
    );
    let response = send(
        &app,
        Method::POST,
        "/api/admin/logout",
        &[
            ("cookie", &cookie),
            ("origin", "https://admin.test"),
            ("x-csrf-token", &csrf),
        ],
        Value::Null,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/admin/devices",
            &[("cookie", &cookie)],
            Value::Null
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    for idle in [true, false] {
        let (cookie, _) = login(&app).await;
        {
            let mut sessions = shared.web.sessions.lock().unwrap();
            let session = sessions.values_mut().next().unwrap();
            if idle {
                session.active -= IDLE;
            } else {
                session.created -= ABSOLUTE;
            }
        }
        assert_eq!(
            send(
                &app,
                Method::GET,
                "/api/admin/session",
                &[("cookie", &cookie)],
                Value::Null
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let (cookie, _) = login(&app).await;
    let response = send(
        &app,
        Method::POST,
        "/api/admin/login",
        &[
            ("cookie", &cookie),
            ("origin", "https://admin.test"),
            ("x-rcs-admin", "1"),
        ],
        json!({"username":"administrator","password":PASSWORD}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap(),
        cookie
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/admin/session",
            &[("cookie", &cookie)],
            Value::Null
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(shared.web.sessions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn authentication_budget_is_bounded_and_cannot_be_bypassed_with_proxy_headers() {
    let (_, app) = app();
    for _ in 0..30 {
        assert_eq!(
            send(
                &app,
                Method::POST,
                "/api/admin/login",
                &[("origin", "https://admin.test"), ("x-rcs-admin", "1")],
                json!({"username":"administrator","password":"incorrect"})
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        send(
            &app,
            Method::POST,
            "/api/admin/login",
            &[
                ("origin", "https://admin.test"),
                ("x-rcs-admin", "1"),
                ("x-forwarded-for", "203.0.113.1")
            ],
            json!({"username":"administrator","password":PASSWORD})
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/devices",
            &[("authorization", &basic("administrator", PASSWORD))],
            Value::Null
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn shared_device_account_and_admin_and_legacy_devices_share_the_same_clipboard() {
    let (shared, app) = app();
    let (cookie, csrf) = login(&app).await;
    let account = create(&app, &cookie, &csrf, "shared-device").await;
    let (_, a, reg) = register(&app, "shared-device", DEVICE_PASSWORD).await;
    let (_, b, _) = register(&app, "shared-device", DEVICE_PASSWORD).await;
    let (_, admin, _) = register(&app, "administrator", PASSWORD).await;
    let (_, legacy, _) = register(&app, "legacy", "fixture-legacy-password").await;
    let id = uuid::Uuid::new_v4().to_string();
    let written = send(
        &app,
        Method::PUT,
        &format!("/api/messages/{id}"),
        &[
            ("authorization", &a),
            ("x-rcs-epoch", reg["epoch"].as_str().unwrap()),
            ("x-rcs-base-revision", "0"),
            ("x-rcs-sensitive", "0"),
        ],
        Value::String("shared text".into()),
    )
    .await;
    assert_eq!(written.status(), StatusCode::OK);
    for token in [&b, &admin, &legacy] {
        let response = send(
            &app,
            Method::GET,
            "/api/state",
            &[("authorization", token)],
            Value::Null,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body(response).await["messageId"], id);
    }
    for token in [&a, &admin, &legacy] {
        assert_eq!(
            send(
                &app,
                Method::GET,
                "/api/admin/accounts",
                &[("authorization", token)],
                Value::Null
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/devices",
            &[("authorization", &basic("shared-device", DEVICE_PASSWORD))],
            Value::Null
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/devices",
            &[("authorization", &basic("legacy", "fixture-legacy-password"))],
            Value::Null
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let hash: String = shared
        .db
        .lock()
        .await
        .query_row(
            "SELECT password_hash FROM rcs_accounts WHERE id=?1",
            [account],
            |r| r.get(0),
        )
        .unwrap();
    assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    assert!(!hash.contains(DEVICE_PASSWORD));
    let list = body(
        send(
            &app,
            Method::GET,
            "/api/admin/accounts",
            &[("cookie", &cookie)],
            Value::Null,
        )
        .await,
    )
    .await
    .to_string();
    assert!(!list.contains(DEVICE_PASSWORD));
    assert!(!list.contains("password_hash"));
    assert!(!list.contains("$argon2"));
}

#[tokio::test]
async fn account_revocation_atomically_removes_all_its_devices_and_wakes_pending_polls() {
    let (_, app) = app();
    let (cookie, csrf) = login(&app).await;
    let account = create(&app, &cookie, &csrf, "shared").await;
    let (_, a, reg) = register(&app, "shared", DEVICE_PASSWORD).await;
    let (_, b, _) = register(&app, "shared", DEVICE_PASSWORD).await;
    let (_, survivor, _) = register(&app, "legacy", "fixture-legacy-password").await;
    let url = format!(
        "/api/changes?epoch={}&after=0",
        reg["epoch"].as_str().unwrap()
    );
    let headers_a = [("authorization", a.as_str())];
    let headers_b = [("authorization", b.as_str())];
    let mut poll_a = std::pin::pin!(send(&app, Method::GET, &url, &headers_a, Value::Null));
    let mut poll_b = std::pin::pin!(send(&app, Method::GET, &url, &headers_b, Value::Null));
    assert!(futures_util::poll!(poll_a.as_mut()).is_pending());
    assert!(futures_util::poll!(poll_b.as_mut()).is_pending());
    let response = send(
        &app,
        Method::DELETE,
        &format!("/api/admin/accounts/{account}"),
        &[
            ("cookie", &cookie),
            ("origin", "https://admin.test"),
            ("x-csrf-token", &csrf),
        ],
        Value::Null,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await["revokedDevices"], 2);
    for poll in [poll_a.as_mut(), poll_b.as_mut()] {
        let std::task::Poll::Ready(response) = futures_util::poll!(poll) else {
            panic!("account revocation must wake every waiting device");
        };
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/state",
            &[("authorization", &survivor)],
            Value::Null
        )
        .await
        .status(),
        StatusCode::OK
    );
    let response = send(
        &app,
        Method::POST,
        "/api/devices",
        &[("authorization", &basic("shared", DEVICE_PASSWORD))],
        json!({"deviceId":uuid::Uuid::new_v4().to_string(),"deviceToken":"ab".repeat(32)}),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "revoked account must not register again"
    );
    assert_eq!(
        body(
            send(
                &app,
                Method::GET,
                "/api/admin/devices",
                &[("cookie", &cookie)],
                Value::Null
            )
            .await
        )
        .await
        .as_array()
        .unwrap()
        .len(),
        1
    );
}

#[tokio::test]
async fn single_device_revoke_does_not_remove_other_devices_in_the_same_account() {
    let (_, app) = app();
    let (cookie, csrf) = login(&app).await;
    create(&app, &cookie, &csrf, "shared").await;
    let (id, a, _) = register(&app, "shared", DEVICE_PASSWORD).await;
    let (_, b, _) = register(&app, "shared", DEVICE_PASSWORD).await;
    assert_eq!(
        send(
            &app,
            Method::DELETE,
            &format!("/api/admin/devices/{id}"),
            &[
                ("cookie", &cookie),
                ("origin", "https://admin.test"),
                ("x-csrf-token", &csrf)
            ],
            Value::Null
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/state",
            &[("authorization", &a)],
            Value::Null
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/state",
            &[("authorization", &b)],
            Value::Null
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn presence_and_labels_are_metadata_only_and_builtin_accounts_cannot_be_deleted() {
    let (_, app) = app();
    let (cookie, csrf) = login(&app).await;
    let (id, token, _) = register(&app, "legacy", "fixture-legacy-password").await;
    let response = body(
        send(
            &app,
            Method::GET,
            "/api/admin/devices",
            &[("cookie", &cookie)],
            Value::Null,
        )
        .await,
    )
    .await;
    assert_eq!(response[0]["online"], true);
    assert!(response[0]["lastSeen"].is_i64());
    send(
        &app,
        Method::GET,
        "/api/state",
        &[("authorization", &token)],
        Value::Null,
    )
    .await;
    let response = body(
        send(
            &app,
            Method::GET,
            "/api/admin/devices",
            &[("cookie", &cookie)],
            Value::Null,
        )
        .await,
    )
    .await;
    assert_eq!(response[0]["online"], true);
    assert!(response[0]["lastSeen"].as_i64().unwrap() > 0);
    let label = "<img src=x onerror=alert(1)>";
    assert_eq!(
        send(
            &app,
            Method::PATCH,
            &format!("/api/admin/devices/{id}"),
            &[
                ("cookie", &cookie),
                ("origin", "https://admin.test"),
                ("x-csrf-token", &csrf)
            ],
            json!({"label":label})
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let response = body(
        send(
            &app,
            Method::GET,
            "/api/admin/devices",
            &[("cookie", &cookie)],
            Value::Null,
        )
        .await,
    )
    .await;
    assert_eq!(response[0]["label"], label);
    assert!(response[0].get("token").is_none());
    assert!(response[0].get("tokenHash").is_none());
    for id in ["@admin", "@legacy"] {
        assert_eq!(
            send(
                &app,
                Method::DELETE,
                &format!("/api/admin/accounts/{id}"),
                &[
                    ("cookie", &cookie),
                    ("origin", "https://admin.test"),
                    ("x-csrf-token", &csrf)
                ],
                Value::Null
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        send(
            &app,
            Method::POST,
            "/api/admin/accounts",
            &[
                ("cookie", &cookie),
                ("origin", "https://admin.test"),
                ("x-csrf-token", &csrf)
            ],
            json!({"username":"administrator","password":DEVICE_PASSWORD})
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn administrative_responses_are_not_cacheable_or_frameable_and_no_secrets_enter_assets() {
    let (_, app) = app();
    for path in [
        "/admin/",
        "/admin/admin.css",
        "/admin/admin.js",
        "/api/admin/devices",
    ] {
        let response = send(&app, Method::GET, path, &[], Value::Null).await;
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains(PASSWORD));
        assert!(!text.contains(DEVICE_PASSWORD));
    }
}

#[tokio::test]
async fn verified_registration_cannot_resurrect_a_deleted_account() {
    let (shared, app) = app();
    let (cookie, csrf) = login(&app).await;
    let account = create(&app, &cookie, &csrf, "race-fixture").await;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        basic("race-fixture", DEVICE_PASSWORD).parse().unwrap(),
    );
    let verified = auth::registration(&headers, &shared).await.unwrap();
    assert!(verified.recheck(&*shared.db.lock().await).is_ok());
    let response = send(
        &app,
        Method::DELETE,
        &format!("/api/admin/accounts/{account}"),
        &[
            ("cookie", &cookie),
            ("origin", "https://admin.test"),
            ("x-csrf-token", &csrf),
        ],
        Value::Null,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let replacement = create(&app, &cookie, &csrf, "race-fixture").await;
    assert_ne!(replacement, account);
    assert_eq!(
        verified.recheck(&*shared.db.lock().await),
        Err(StatusCode::UNAUTHORIZED),
        "even recreating the same username must not resurrect old in-flight registration"
    );
    assert!(
        shared
            .auth
            .check_names(&*shared.db.lock().await, "race-fixture")
            .is_err()
    );
}

#[tokio::test]
async fn management_limits_and_unknown_privilege_fields_fail_closed() {
    let (shared, app) = app();
    let (cookie, csrf) = login(&app).await;
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", "https://admin.test"),
        ("x-csrf-token", csrf.as_str()),
    ];
    for input in [
        json!({"username":"device","password":DEVICE_PASSWORD,"role":"admin"}),
        json!({"username":"<script>","password":DEVICE_PASSWORD}),
        json!({"username":"device","password":"short"}),
    ] {
        assert!(
            send(&app, Method::POST, "/api/admin/accounts", &headers, input)
                .await
                .status()
                .is_client_error()
        );
    }
    let huge = send(
        &app,
        Method::POST,
        "/api/admin/accounts",
        &headers,
        json!({"username":"device","password":"x".repeat(33000)}),
    )
    .await;
    assert_eq!(huge.status(), StatusCode::PAYLOAD_TOO_LARGE);
    for _ in 0..20 {
        login(&app).await;
    }
    assert_eq!(shared.web.sessions.lock().unwrap().len(), MAX_SESSIONS);
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/admin/accounts",
            &[("cookie", &cookie)],
            Value::Null
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
        "oldest browser session must be evicted at the cap"
    );
}

#[tokio::test]
async fn maximum_password_survives_json_escaping_and_pairing() {
    let (_, app) = app();
    let (cookie, csrf) = login(&app).await;
    // Each byte is serialized as six bytes (\\u0001); still a valid 4096-byte password.
    let password = "\u{0001}".repeat(4096);
    let response = send(
        &app,
        Method::POST,
        "/api/admin/accounts",
        &[
            ("cookie", &cookie),
            ("origin", "https://admin.test"),
            ("x-csrf-token", &csrf),
        ],
        json!({"username":"escaped-password","password":password}),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "JSON escaping must not reduce the advertised password limit"
    );
    register(&app, "escaped-password", &password).await;
}

#[test]
fn schema_upgrade_preserves_existing_tokens_cursors_and_metadata() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE v1_devices(id TEXT PRIMARY KEY,token_hash BLOB NOT NULL,ack_revision INTEGER NOT NULL); INSERT INTO v1_devices VALUES('existing-device',x'01020304',7);").unwrap();
    crate::api::init(&db).unwrap();
    crate::api::init(&db).unwrap();
    let row:(Vec<u8>,i64,Option<String>,String,Option<i64>)=db.query_row("SELECT token_hash,ack_revision,account_id,label,last_seen FROM v1_devices WHERE id='existing-device'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
    assert_eq!(row, (vec![1, 2, 3, 4], 7, None, String::new(), None));
    let tables: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='rcs_accounts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
}

#[tokio::test]
async fn backfill_setting_requires_admin_csrf_valid_integer_and_audits_changes() {
    let (shared, app) = app();
    let (cookie, csrf) = login(&app).await;
    let path = "/api/admin/sync-settings";
    assert_eq!(
        send(&app, Method::GET, path, &[], Value::Null)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            Method::PUT,
            path,
            &[("cookie", &cookie), ("origin", "https://admin.test")],
            json!({"backfillWindowMinutes":0})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", "https://admin.test"),
        ("x-csrf-token", csrf.as_str()),
    ];
    for value in [json!(-1), json!(1441), json!(1.5), json!("5")] {
        assert!(
            send(
                &app,
                Method::PUT,
                path,
                &headers,
                json!({"backfillWindowMinutes":value})
            )
            .await
            .status()
            .is_client_error()
        );
    }
    assert_eq!(
        send(
            &app,
            Method::PUT,
            path,
            &headers,
            json!({"backfillWindowMinutes":0})
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let result =
        body(send(&app, Method::GET, path, &[("cookie", &cookie)], Value::Null).await).await;
    assert_eq!(result["backfillWindowMinutes"], 0);
    assert_eq!(result["audit"][0]["actor"], "administrator");
    assert_eq!(result["audit"][0]["oldValue"], 5);
    assert_eq!(result["audit"][0]["newValue"], 0);
    shared
        .db
        .lock()
        .await
        .execute("DELETE FROM rcs_settings", [])
        .unwrap();
    let result =
        body(send(&app, Method::GET, path, &[("cookie", &cookie)], Value::Null).await).await;
    assert_eq!(result["backfillWindowMinutes"], 0);
    assert_eq!(result["configurationValid"], false);
}

#[tokio::test]
async fn corrupt_text_window_can_be_repaired_by_an_administrator() {
    let (shared, app) = app();
    let (cookie, csrf) = login(&app).await;
    shared
        .db
        .lock()
        .await
        .execute("UPDATE rcs_settings SET value='invalid'", [])
        .unwrap();
    let response = send(
        &app,
        Method::GET,
        "/api/admin/sync-settings",
        &[("cookie", &cookie)],
        Value::Null,
    )
    .await;
    assert_eq!(body(response).await["backfillWindowMinutes"], 0);
    let response = send(
        &app,
        Method::PUT,
        "/api/admin/sync-settings",
        &[
            ("cookie", &cookie),
            ("origin", "https://admin.test"),
            ("x-csrf-token", &csrf),
        ],
        json!({"backfillWindowMinutes":5}),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "invalid SQLite type must not prevent administrator repair"
    );
    let value = body(
        send(
            &app,
            Method::GET,
            "/api/admin/sync-settings",
            &[("cookie", &cookie)],
            Value::Null,
        )
        .await,
    )
    .await;
    assert_eq!(value["backfillWindowMinutes"], 5);
    assert_eq!(value["audit"][0]["oldValue"], Value::Null);
}

#[tokio::test]
async fn device_sync_diagnostics_are_effective_read_only_and_redacted() {
    let (shared, app) = app();
    let (cookie, _) = login(&app).await;
    let now = crate::presence::unix_time();
    let cases = [
        (None, false, "", None, 0, "unknown", false),
        (
            Some("PUSH_ONLY"),
            false,
            "update",
            Some(now + 300),
            now + 100,
            "reported",
            true,
        ),
        (
            Some("RECEIVE_ONLY"),
            false,
            "update",
            Some(now + 300),
            now + 100,
            "reported",
            true,
        ),
        (
            Some("BIDIRECTIONAL"),
            false,
            "update",
            Some(now + 300),
            now + 100,
            "reported",
            true,
        ),
        (
            Some("BIDIRECTIONAL"),
            true,
            "update",
            Some(now + 300),
            now + 100,
            "reported",
            false,
        ),
        (
            Some("RECEIVE_ONLY"),
            false,
            "update",
            Some(now - 300),
            now + 100,
            "expired",
            false,
        ),
        (None, false, "expired", Some(now - 300), 0, "expired", false),
        (None, false, "revoke", None, 0, "revoked", false),
        (
            Some("RECEIVE_ONLY"),
            false,
            "update",
            Some(now + 300),
            now - 100,
            "reported",
            false,
        ),
    ];
    {
        let db = shared.db.lock().await;
        for (i, (direction, paused, op, expires, lease, _, _)) in cases.iter().enumerate() {
            let id = format!("00000000-0000-4000-8000-{i:012}");
            db.execute("INSERT INTO v1_devices(id,token_hash,ack_revision,last_seen) VALUES(?1,x'012345',7,?2)", params![id, now - 500]).unwrap();
            if i > 0 {
                db.execute("INSERT INTO rcs_direction(device_id,session_id,version,direction,paused,updated,expires,lease_until,last_op,last_payload,source) VALUES(?1,'private-session',9,?2,?3,?4,?5,?6,?7,'private-payload','WS')",params![id,direction,paused,now-100,expires,lease,op]).unwrap();
            }
        }
        db.execute_batch("CREATE TABLE admin_test_writes(n INTEGER); INSERT INTO admin_test_writes VALUES(0);
            CREATE TRIGGER admin_test_direction AFTER UPDATE ON rcs_direction BEGIN UPDATE admin_test_writes SET n=n+1; END;
            CREATE TRIGGER admin_test_device AFTER UPDATE ON v1_devices BEGIN UPDATE admin_test_writes SET n=n+1; END;").unwrap();
    }
    for _ in 0..2 {
        let response = send(
            &app,
            Method::GET,
            "/api/admin/devices",
            &[("cookie", &cookie)],
            Value::Null,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let devices = body(response).await;
        for (i, (direction, paused, _, _, _, state, lease)) in cases.iter().enumerate() {
            let sync = &devices[i]["sync"];
            assert_eq!(
                sync["reportState"], *state,
                "case {i}: effective report state"
            );
            assert_eq!(
                sync["direction"],
                if *state == "reported" {
                    json!(direction)
                } else {
                    Value::Null
                },
                "case {i}: effective direction"
            );
            assert_eq!(sync["paused"], *paused, "case {i}: pause");
            assert_eq!(sync["dataLeaseActive"], *lease, "case {i}: data lease");
            assert_eq!(
                devices[i]["online"], false,
                "read must not mark a device online"
            );
            assert_eq!(devices[i]["lastSeen"], now - 500);
            assert_eq!(devices[i]["ackRevision"], 7);
            if i > 0 {
                assert_eq!(sync["version"], 9);
                assert_eq!(sync["lastReportSource"], "ws");
                assert_eq!(sync["reportedAt"], now - 100);
            }
        }
        let encoded = devices.to_string();
        for secret in [
            "private-session",
            "private-payload",
            "tokenHash",
            "sessionId",
            "password",
        ] {
            assert!(!encoded.contains(secret), "must not expose {secret}");
        }
    }
    assert_eq!(
        shared
            .db
            .lock()
            .await
            .query_row("SELECT n FROM admin_test_writes", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0,
        "viewing diagnostics must not refresh TTL, cursors or presence"
    );
}

#[tokio::test]
async fn device_credentials_cannot_read_admin_diagnostics() {
    let (_, app) = app();
    let (_, bearer, _) = register(&app, "legacy", "fixture-legacy-password").await;
    for path in ["/api/admin/devices", "/api/admin/sync-settings"] {
        for authorization in [
            &bearer,
            &basic("legacy", "fixture-legacy-password"),
            &basic("administrator", PASSWORD),
        ] {
            assert_eq!(
                send(
                    &app,
                    Method::GET,
                    path,
                    &[("authorization", authorization)],
                    Value::Null
                )
                .await
                .status(),
                StatusCode::UNAUTHORIZED,
                "{path} must require a browser administrator session"
            );
        }
    }
}
