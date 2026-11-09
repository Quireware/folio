use rusqlite::Connection;

use crate::StorageError;

pub(crate) fn item_ids(conn: &Connection) -> Result<Vec<String>, StorageError> {
    let mut statement = conn.prepare("SELECT id FROM items ORDER BY id")?;
    let rows = statement.query_map([], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

pub(crate) fn settings(conn: &Connection) -> Result<Vec<(String, String)>, StorageError> {
    let mut statement = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

pub(crate) fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

pub(crate) fn server_ids(conn: &Connection) -> Result<Vec<String>, StorageError> {
    let mut statement = conn.prepare("SELECT id FROM servers ORDER BY id")?;
    let rows = statement.query_map([], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}
