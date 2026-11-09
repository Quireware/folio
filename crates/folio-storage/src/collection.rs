//! Collections, tags, ratings, smart rules, and history.

use rusqlite::{params, Connection};

use crate::{Database, StorageError};

/// Create a named collection.
///
/// # Errors
///
/// Returns an error when an id or the name is empty, or the writer cannot commit.
pub fn create_collection(db: &Database, id: &str, name: &str) -> Result<(), StorageError> {
    let id = owned_id(id)?;
    let name = owned_id(name)?;
    db.work(move |conn| {
        conn.execute(
            "INSERT INTO collections (id, name) VALUES (?1, ?2)",
            params![id, name],
        )?;
        Ok(())
    })
}

/// Add an item to a collection.
///
/// # Errors
///
/// Returns an error when an id is empty or the writer cannot commit.
pub fn add_collection_item(
    db: &Database,
    collection_id: &str,
    item_id: &str,
) -> Result<(), StorageError> {
    let collection_id = owned_id(collection_id)?;
    let item_id = owned_id(item_id)?;
    db.work(move |conn| {
        conn.execute(
            "INSERT INTO collection_items (collection_id, item_id) VALUES (?1, ?2)",
            params![collection_id, item_id],
        )?;
        Ok(())
    })
}

/// Remove an item from a collection.
///
/// # Errors
///
/// Returns an error when the row is missing or the writer cannot commit.
pub fn remove_collection_item(
    db: &Database,
    collection_id: &str,
    item_id: &str,
) -> Result<(), StorageError> {
    let collection_id = owned_id(collection_id)?;
    let item_id = owned_id(item_id)?;
    db.work(move |conn| {
        let changed = conn.execute(
            "DELETE FROM collection_items WHERE collection_id = ?1 AND item_id = ?2",
            params![collection_id, item_id],
        )?;
        if changed != 1 {
            return Err(StorageError::Missing);
        }
        Ok(())
    })
}

/// Item ids in a collection, in id order.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn collection_items(db: &Database, collection_id: &str) -> Result<Vec<String>, StorageError> {
    let collection_id = collection_id.to_owned();
    db.work(move |conn| {
        let mut statement = conn.prepare(
            "SELECT item_id FROM collection_items WHERE collection_id = ?1 ORDER BY item_id",
        )?;
        let rows = statement.query_map([collection_id], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    })
}

/// Attach a tag.
///
/// # Errors
///
/// Returns an error when the book id or tag is empty, or the writer cannot commit.
pub fn add_tag(db: &Database, book_id: &str, tag: &str) -> Result<(), StorageError> {
    let book_id = owned_id(book_id)?;
    let tag = owned_id(tag)?;
    db.work(move |conn| {
        conn.execute(
            "INSERT INTO tags (book_id, tag) VALUES (?1, ?2)",
            params![book_id, tag],
        )?;
        Ok(())
    })
}

/// Tags for one book, in tag order.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn tags(db: &Database, book_id: &str) -> Result<Vec<String>, StorageError> {
    let book_id = book_id.to_owned();
    db.work(move |conn| {
        let mut statement = conn.prepare("SELECT tag FROM tags WHERE book_id = ?1 ORDER BY tag")?;
        let rows = statement.query_map([book_id], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    })
}

/// Store a half-star rating. `halves` is 1..=10.
///
/// # Errors
///
/// Returns an error when `halves` is outside 1..=10 or the writer cannot commit.
pub fn set_rating(db: &Database, book_id: &str, halves: u8) -> Result<(), StorageError> {
    let book_id = owned_id(book_id)?;
    if !(1..=10).contains(&halves) {
        return Err(StorageError::Rating);
    }
    let halves = i64::from(halves);
    db.work(move |conn| {
        conn.execute(
            "INSERT INTO ratings (book_id, halves) VALUES (?1, ?2)
             ON CONFLICT(book_id) DO UPDATE SET halves = excluded.halves",
            params![book_id, halves],
        )?;
        Ok(())
    })
}

/// Half-star rating, when one is stored.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn rating(db: &Database, book_id: &str) -> Result<Option<u8>, StorageError> {
    let book_id = book_id.to_owned();
    db.work(move |conn| {
        match conn.query_row(
            "SELECT halves FROM ratings WHERE book_id = ?1",
            [book_id],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(halves) => u8::try_from(halves)
                .map(Some)
                .map_err(|_| StorageError::EmptyId),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    })
}

/// One smart-collection rule.
///
/// # Errors
///
/// Returns an error when a field is empty or the writer cannot commit.
pub fn set_smart_rule(
    db: &Database,
    collection_id: &str,
    field: &str,
    expected: &str,
) -> Result<(), StorageError> {
    let collection_id = owned_id(collection_id)?;
    let field = owned_id(field)?;
    let expected = owned_id(expected)?;
    db.work(move |conn| {
        conn.execute(
            "INSERT INTO collections (id, name) VALUES (?1, ?1)
             ON CONFLICT(id) DO NOTHING",
            [&collection_id],
        )?;
        conn.execute(
            "INSERT INTO smart_rules (collection_id, field, expected) VALUES (?1, ?2, ?3)
             ON CONFLICT(collection_id) DO UPDATE SET field = excluded.field, expected = excluded.expected",
            params![collection_id, field, expected],
        )?;
        Ok(())
    })
}

/// The smart rule for a collection.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn smart_rule(
    db: &Database,
    collection_id: &str,
) -> Result<Option<(String, String)>, StorageError> {
    let collection_id = collection_id.to_owned();
    db.work(move |conn| {
        match conn.query_row(
            "SELECT field, expected FROM smart_rules WHERE collection_id = ?1",
            [collection_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ) {
            Ok(rule) => Ok(Some(rule)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    })
}

/// Record that a book was opened. This does not change Continue Reading.
///
/// # Errors
///
/// Returns an error when `book_id` is empty or the writer cannot commit.
pub fn open_history(db: &Database, book_id: &str, opened_at: i64) -> Result<(), StorageError> {
    let book_id = owned_id(book_id)?;
    db.work(move |conn| {
        conn.execute(
            "INSERT INTO history (book_id, opened_at) VALUES (?1, ?2)
             ON CONFLICT(book_id) DO UPDATE SET opened_at = excluded.opened_at",
            params![book_id, opened_at],
        )?;
        Ok(())
    })
}

/// History rows, newest first.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn history(db: &Database) -> Result<Vec<(String, i64)>, StorageError> {
    db.work(|conn| {
        let mut statement = conn
            .prepare("SELECT book_id, opened_at FROM history ORDER BY opened_at DESC, book_id")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    })
}

/// Remove one history row.
///
/// # Errors
///
/// Returns an error when the row is missing or the writer cannot commit.
pub fn clear_history_one(db: &Database, book_id: &str) -> Result<(), StorageError> {
    let book_id = owned_id(book_id)?;
    db.work(move |conn| one_row(conn, "DELETE FROM history WHERE book_id = ?1", &book_id))
}

/// Remove every history row.
///
/// # Errors
///
/// Returns an error when the writer cannot commit.
pub fn clear_history(db: &Database) -> Result<(), StorageError> {
    db.work(|conn| {
        conn.execute("DELETE FROM history", [])?;
        Ok(())
    })
}

fn one_row(conn: &Connection, sql: &str, id: &str) -> Result<(), StorageError> {
    let changed = conn.execute(sql, [id])?;
    if changed != 1 {
        return Err(StorageError::Missing);
    }
    Ok(())
}

fn owned_id(value: &str) -> Result<String, StorageError> {
    if value.is_empty() {
        Err(StorageError::EmptyId)
    } else {
        Ok(value.to_owned())
    }
}
