use std::{error::Error, fs, io::Read, path::Path};

use rusqlite::{Connection, OpenFlags, params};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn read_key() -> Result<String> {
    let key = std::env::var("RCS_DB_KEY")
        .map_err(|_| "RCS_DB_KEY is required and must be valid UTF-8")?;
    validate_key(&key)?;
    Ok(key)
}

fn validate_key(key: &str) -> Result<()> {
    if key.len() != 64 || !key.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("database key must be exactly 64 hexadecimal characters".into());
    }
    Ok(())
}

fn apply_key(db: &Connection, key: &str) -> Result<()> {
    validate_key(key)?;
    let version: String = db.pragma_query_value(None, "cipher_version", |row| row.get(0))?;
    if version.is_empty() {
        return Err("SQLCipher is unavailable".into());
    }
    db.pragma_update(None, "key", format!("x'{key}'"))?;
    // Force the first page to be decrypted now, before creating any schema.
    db.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })?;
    Ok(())
}

fn data_summary(db: &Connection) -> rusqlite::Result<Vec<(i64, i64)>> {
    [
        "SELECT count(*), coalesce(sum(length(profile)), 0) FROM current",
        "SELECT count(*), coalesce(sum(length(profile) + length(data)), 0) FROM history",
        "SELECT count(*), coalesce(sum(length(data)), 0) FROM pending",
    ]
    .iter()
    .map(|query| db.query_row(query, [], |row| Ok((row.get(0)?, row.get(1)?))))
    .collect()
}

pub fn open(path: &Path, key: &str) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !path.exists() && path.with_extension("db.plaintext-backup").exists() {
        return Err(
            "database is missing while a migration backup exists; manual recovery required".into(),
        );
    }
    let db = Connection::open(path)?;
    apply_key(&db, key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000;
        CREATE TABLE IF NOT EXISTS current(id INTEGER PRIMARY KEY CHECK(id=1), profile TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS history(hash TEXT PRIMARY KEY, profile TEXT NOT NULL, data BLOB NOT NULL);
        CREATE TABLE IF NOT EXISTS pending(name TEXT PRIMARY KEY, hash TEXT NOT NULL, data BLOB NOT NULL, created INTEGER NOT NULL);
        DELETE FROM pending WHERE created < unixepoch()-3600;")?;
    crate::v1::init(&db)?;
    Ok(db)
}

/// Offline migration. Caller must stop all processes using the database first.
pub fn migrate(path: &Path, key: &str) -> Result<()> {
    validate_key(key)?;
    let backup = path.with_extension("db.plaintext-backup");
    let tmp = path.with_extension("db.encrypted-tmp");
    if backup.exists() || tmp.exists() {
        return Err(
            "migration backup or temporary file already exists; manual inspection required".into(),
        );
    }
    let mut header = [0u8; 16];
    fs::File::open(path)?.read_exact(&mut header)?;
    if &header != b"SQLite format 3\0" {
        return Err("database is not a plaintext SQLite database".into());
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let integrity: String = db.pragma_query_value(None, "integrity_check", |r| r.get(0))?;
    if integrity != "ok" {
        return Err("source database integrity check failed".into());
    }
    // A stopped service is required; a busy checkpoint must abort migration.
    let (busy, _, _): (i64, i64, i64) =
        db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    if busy != 0 {
        return Err("database checkpoint is busy; stop the service".into());
    }
    let mode: String = db.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        let changed: String = db.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
        if !changed.eq_ignore_ascii_case("delete") {
            return Err("could not leave WAL mode".into());
        }
    }
    let source_summary = data_summary(&db)?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    let tmp_str = tmp.to_str().ok_or("database path is not UTF-8")?;
    db.execute(
        &format!("ATTACH DATABASE ?1 AS encrypted KEY \"x'{key}'\""),
        params![tmp_str],
    )?;
    db.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))?;
    db.execute_batch("DETACH DATABASE encrypted")?;
    drop(db);
    let encrypted = Connection::open_with_flags(&tmp, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    apply_key(&encrypted, key)?;
    let integrity: String = encrypted.pragma_query_value(None, "integrity_check", |r| r.get(0))?;
    if integrity != "ok" {
        return Err("encrypted database integrity check failed".into());
    }
    if data_summary(&encrypted)? != source_summary {
        return Err("encrypted database does not match source data summary".into());
    }
    drop(encrypted);
    fs::File::open(&tmp)?.sync_all()?;
    fs::rename(path, &backup)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))?;
    }
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::rename(&backup, path);
        return Err(err.into());
    }
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_environment_key_format() {
        assert!(validate_key(&"ab".repeat(32)).is_ok());
        assert!(validate_key(&"ab".repeat(31)).is_err());
        assert!(validate_key(&"zz".repeat(32)).is_err());
        assert!(validate_key(&("ab".repeat(32) + "\n")).is_err());
    }

    #[test]
    fn encrypts_new_database_and_rejects_wrong_key() {
        let dir = std::env::temp_dir().join(format!("rcs-db-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("clipsync.db");
        let key = "ab".repeat(32);
        {
            let db = open(&path, &key).unwrap();
            db.execute(
                "INSERT INTO current VALUES (1, ?1)",
                ["private marker 123456789"],
            )
            .unwrap();
            db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        }
        assert!(
            !fs::read(&path)
                .unwrap()
                .windows(24)
                .any(|v| v == b"private marker 123456789")
        );
        assert!(open(&path, &"cd".repeat(32)).is_err());
        assert_eq!(
            open(&path, &key)
                .unwrap()
                .query_row("SELECT profile FROM current", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "private marker 123456789"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrates_plaintext_and_retains_backup() {
        let dir = std::env::temp_dir().join(format!("rcs-migrate-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("clipsync.db");
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE current(id INTEGER PRIMARY KEY, profile TEXT NOT NULL); INSERT INTO current VALUES(1,'migration sample'); CREATE TABLE history(hash TEXT PRIMARY KEY, profile TEXT NOT NULL, data BLOB NOT NULL); INSERT INTO history VALUES('sample','profile',X'010203'); CREATE TABLE pending(name TEXT PRIMARY KEY, hash TEXT NOT NULL, data BLOB NOT NULL, created INTEGER NOT NULL); INSERT INTO pending VALUES('test.txt','sample',X'0405',unixepoch());").unwrap();
        }
        let key = "11".repeat(32);
        assert!(open(&path, &key).is_err());
        migrate(&path, &key).unwrap();
        assert!(path.with_extension("db.plaintext-backup").exists());
        assert_eq!(
            open(&path, &key)
                .unwrap()
                .query_row("SELECT profile FROM current", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "migration sample"
        );
        let db = open(&path, &key).unwrap();
        assert_eq!(
            db.query_row("SELECT data FROM history WHERE hash='sample'", [], |r| r
                .get::<_, Vec<u8>>(0))
                .unwrap(),
            [1, 2, 3]
        );
        assert_eq!(
            db.query_row("SELECT data FROM pending WHERE name='test.txt'", [], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .unwrap(),
            [4, 5]
        );
        drop(db);
        assert!(open(&path, &"22".repeat(32)).is_err());
        assert!(migrate(&path, &key).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
