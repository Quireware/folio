//! JSON backup of non-secret library state.
//!
//! A tree grant is a URI, not a file copied onto the new device.

use std::path::Path;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::catalog::{self, CatalogItem};
use crate::library;
use crate::{Database, StorageError};

/// Backup schema this build reads and writes.
pub const SCHEMA_VERSION: u32 = 3;

/// One export. Secrets stay `secret_ref` strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    /// Schema version.
    pub schema: u32,
    /// Library rows.
    pub items: Vec<CatalogItem>,
    /// Progress rows.
    pub progress: Vec<ProgressBackup>,
    /// Settings, including device grant URIs.
    pub settings: Vec<SettingBackup>,
    /// Servers. `secret_ref` is a key, not a password or cookie.
    pub servers: Vec<ServerBackup>,
}

/// One progress row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressBackup {
    /// Book id.
    pub book_id: String,
    /// Locator.
    pub locator: String,
    /// Unix milliseconds.
    pub updated_at: i64,
    /// Page, when known.
    pub page: Option<i64>,
    /// Chapter, when known.
    pub chapter: Option<String>,
    /// Completion flag.
    pub complete: bool,
}

/// One setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingBackup {
    /// Key.
    pub key: String,
    /// Value.
    pub value: String,
}

/// One server, without a raw secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerBackup {
    /// Id.
    pub id: String,
    /// Protocol.
    pub kind: String,
    /// Base URL.
    pub url: String,
    /// Display name.
    pub display_name: String,
    /// Secret-store key.
    pub secret_ref: Option<String>,
}

/// Read a backup from `db`.
///
/// # Errors
///
/// Returns an error when the writer cannot read or the JSON cannot be encoded.
pub fn export_backup(db: &Database) -> Result<String, StorageError> {
    let backup = db.work(read_backup)?;
    serde_json::to_string(&backup).map_err(|_| StorageError::Schema)
}

/// Replace library, progress, settings, and servers from `json`.
///
/// A schema other than [`SCHEMA_VERSION`] writes nothing.
///
/// # Errors
///
/// Returns an error when the schema is wrong or a write fails. A failed write is rolled back.
pub fn import_backup(db: &Database, json: &str) -> Result<(), StorageError> {
    let backup: Backup = serde_json::from_str(json).map_err(|_| StorageError::Schema)?;
    if backup.schema != SCHEMA_VERSION {
        return Err(StorageError::Schema);
    }
    db.work(move |conn| {
        let tx = conn.transaction()?;
        for item in &backup.items {
            catalog::upsert(&tx, item)?;
        }
        for row in &backup.progress {
            let complete = if row.complete { 1 } else { 0 };
            tx.execute(
                "INSERT INTO progress (book_id, locator, updated_at, page, chapter, complete)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(book_id) DO UPDATE SET
                    locator = excluded.locator,
                    updated_at = excluded.updated_at,
                    page = excluded.page,
                    chapter = excluded.chapter,
                    complete = excluded.complete",
                params![
                    row.book_id,
                    row.locator,
                    row.updated_at,
                    row.page,
                    row.chapter,
                    complete
                ],
            )?;
        }
        for setting in &backup.settings {
            library::set_setting(&tx, &setting.key, &setting.value)?;
        }
        for server in &backup.servers {
            tx.execute(
                "INSERT INTO servers (id, kind, url, display_name, secret_ref)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    kind = excluded.kind,
                    url = excluded.url,
                    display_name = excluded.display_name,
                    secret_ref = excluded.secret_ref",
                params![
                    server.id,
                    server.kind,
                    server.url,
                    server.display_name,
                    server.secret_ref
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
}

/// Setting value, or `"off"` when the key is absent.
pub fn flag_or_off<'a>(settings: &'a [(String, String)], key: &str) -> &'a str {
    settings
        .iter()
        .find(|(stored, _)| stored == key)
        .map(|(_, value)| value.as_str())
        .unwrap_or("off")
}

/// A content URI or any other non-file is not a book copied onto this device.
pub fn is_copied_file(uri: &str) -> bool {
    uri.starts_with("file:") && Path::new(uri.trim_start_matches("file:")).is_file()
}

/// Write `text` to `path`.
///
/// # Errors
///
/// Returns an error when the file cannot be created.
pub fn write_log(path: &Path, text: &str) -> Result<(), StorageError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

fn read_backup(conn: &mut Connection) -> Result<Backup, StorageError> {
    let settings = library::settings(conn)?
        .into_iter()
        .map(|(key, value)| SettingBackup { key, value })
        .collect();
    let progress = {
        let mut statement = conn.prepare(
            "SELECT book_id, locator, updated_at, page, chapter, COALESCE(complete, 0)
             FROM progress ORDER BY book_id",
        )?;
        let rows = statement.query_map([], |row| {
            let complete: i64 = row.get(5)?;
            Ok(ProgressBackup {
                book_id: row.get(0)?,
                locator: row.get(1)?,
                updated_at: row.get(2)?,
                page: row.get(3)?,
                chapter: row.get(4)?,
                complete: complete != 0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let servers = {
        let mut statement = conn
            .prepare("SELECT id, kind, url, display_name, secret_ref FROM servers ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok(ServerBackup {
                id: row.get(0)?,
                kind: row.get(1)?,
                url: row.get(2)?,
                display_name: row.get(3)?,
                secret_ref: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    Ok(Backup {
        schema: SCHEMA_VERSION,
        items: catalog::items(conn)?,
        progress,
        settings,
        servers,
    })
}
