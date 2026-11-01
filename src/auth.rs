//! Separate administrator and sync-only account credentials. Never log inputs.
use crate::{Shared, db_error};
use argon2::{
    Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version,
    password_hash::{PasswordHash, SaltString},
};
use axum::http::{HeaderMap, StatusCode, header};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{Connection, OptionalExtension};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

pub const ADMIN_ACCOUNT: &str = "@admin";
pub const MAX_ACCOUNTS: i64 = 64;

pub struct Credentials {
    pub user: String,
    password: String,
}
impl Credentials {
    pub fn new(user: String, password: String) -> Result<Self, &'static str> {
        if user.is_empty()
            || user.len() > 128
            || user.contains([':', '\r', '\n'])
            || user.chars().any(char::is_control)
            || !(12..=4096).contains(&password.len())
        {
            return Err(
                "invalid administrator username or password length (12–4096 bytes required)",
            );
        }
        Ok(Self { user, password })
    }
    pub fn matches(&self, user: &str, password: &str) -> bool {
        bool::from(
            user.as_bytes().ct_eq(self.user.as_bytes())
                & password.as_bytes().ct_eq(self.password.as_bytes()),
        )
    }
}

pub struct AuthState {
    pub admin: Option<Credentials>,
    attempts: Mutex<VecDeque<Instant>>,
    pub password_work: Arc<Semaphore>,
}
impl Default for AuthState {
    fn default() -> Self {
        Self::new(None)
    }
}
impl AuthState {
    pub fn new(admin: Option<Credentials>) -> Self {
        Self {
            admin,
            attempts: Mutex::new(VecDeque::new()),
            password_work: Arc::new(Semaphore::new(1)),
        }
    }
    pub fn from_env(legacy_user: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let user = std::env::var("RCS_ADMIN_USER")
            .ok()
            .filter(|s| !s.is_empty());
        let file = std::env::var("RCS_ADMIN_PASSWORD_FILE")
            .ok()
            .filter(|s| !s.is_empty());
        let password = std::env::var("RCS_ADMIN_PASSWORD")
            .ok()
            .filter(|s| !s.is_empty());
        if user.is_none() && file.is_none() && password.is_none() {
            return Ok(Self::default());
        }
        let user = user.ok_or("RCS_ADMIN_USER is required with administrator credentials")?;
        if user == legacy_user {
            return Err("RCS_ADMIN_USER must differ from RCS_USER".into());
        }
        let password = if let Some(file) = file {
            std::fs::read_to_string(file)?
                .trim_end_matches(['\r', '\n'])
                .to_owned()
        } else {
            password.ok_or("RCS_ADMIN_PASSWORD or RCS_ADMIN_PASSWORD_FILE is required")?
        };
        Ok(Self::new(Some(Credentials::new(user, password)?)))
    }
    pub fn check_names(
        &self,
        db: &Connection,
        legacy: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for name in [Some(legacy), self.admin.as_ref().map(|a| a.user.as_str())]
            .into_iter()
            .flatten()
        {
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM rcs_accounts WHERE username=?1)",
                [name],
                |r| r.get(0),
            )?;
            if exists {
                return Err(
                    "environment account name conflicts with an existing managed device account"
                        .into(),
                );
            }
        }
        Ok(())
    }
    // Bounded global budget includes login AND Basic endpoints, so an alternate
    // endpoint or forged proxy IP cannot bypass password-attempt throttling.
    // Device bearer authentication never consumes this budget.
    pub fn attempt(&self) -> Result<(), StatusCode> {
        let now = Instant::now();
        let mut attempts = self
            .attempts
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        while attempts
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
        {
            attempts.pop_front();
        }
        if attempts.len() >= 30 {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        attempts.push_back(now);
        Ok(())
    }
}

pub fn basic(headers: &HeaderMap) -> Result<(String, String), StatusCode> {
    let encoded = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.strip_prefix("Basic ")
                .or_else(|| s.strip_prefix("basic "))
        })
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if encoded.len() > 8192 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let value = String::from_utf8(bytes).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let (user, password) = value.split_once(':').ok_or(StatusCode::UNAUTHORIZED)?;
    if user.len() > 512 || password.len() > 4096 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok((user.to_owned(), password.to_owned()))
}

pub fn require_admin(headers: &HeaderMap, app: &Shared) -> Result<(), StatusCode> {
    app.auth.attempt()?;
    let (user, password) = basic(headers)?;
    let valid = if let Some(admin) = &app.auth.admin {
        admin.matches(&user, &password)
    } else {
        bool::from(
            user.as_bytes().ct_eq(app.user.as_bytes())
                & password.as_bytes().ct_eq(app.password.as_bytes()),
        )
    };
    if valid {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

pub struct RegistrationAccount {
    pub id: Option<String>,
    password_hash: Option<String>,
}
impl RegistrationAccount {
    // Recheck inside the registration transaction: account deletion or password
    // replacement after verification must not resurrect an account's devices.
    pub fn recheck(&self, db: &Connection) -> Result<(), StatusCode> {
        if let Some(hash) = &self.password_hash {
            let current: Option<String> = db
                .query_row(
                    "SELECT password_hash FROM rcs_accounts WHERE id=?1",
                    [&self.id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_error)?;
            if current.as_ref() != Some(hash) {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        Ok(())
    }
}

pub async fn registration(
    headers: &HeaderMap,
    app: &Shared,
) -> Result<RegistrationAccount, StatusCode> {
    app.auth.attempt()?;
    let (user, password) = basic(headers)?;
    if app
        .auth
        .admin
        .as_ref()
        .is_some_and(|a| a.matches(&user, &password))
    {
        return Ok(RegistrationAccount {
            id: Some(ADMIN_ACCOUNT.into()),
            password_hash: None,
        });
    }
    if bool::from(
        user.as_bytes().ct_eq(app.user.as_bytes())
            & password.as_bytes().ct_eq(app.password.as_bytes()),
    ) {
        return Ok(RegistrationAccount {
            id: None,
            password_hash: None,
        });
    }
    let account: Option<(String, String)> = app
        .db
        .lock()
        .await
        .query_row(
            "SELECT id,password_hash FROM rcs_accounts WHERE username=?1",
            [&user],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    let Some((id, hash)) = account else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let saved = hash.clone();
    let permit = Arc::clone(&app.auth.password_work)
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        PasswordHash::new(&hash).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !valid {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(RegistrationAccount {
        id: Some(id),
        password_hash: Some(saved),
    })
}

pub fn random_hex() -> Result<String, StatusCode> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub async fn password_hash(app: &Shared, password: String) -> Result<String, StatusCode> {
    if !(12..=4096).contains(&password.len()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let permit = Arc::clone(&app.auth.password_work)
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let salt = SaltString::encode_b64(&salt).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let params =
            Params::new(19 * 1024, 2, 1, None).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}
