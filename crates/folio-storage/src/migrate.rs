use rusqlite::Connection;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS items (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    source TEXT NOT NULL,
    file_type TEXT NOT NULL,
    label TEXT,
    size INTEGER,
    modified_ms INTEGER,
    series_guess TEXT,
    series TEXT,
    page_count INTEGER,
    image_child_count INTEGER,
    fingerprint TEXT,
    enriched_fingerprint TEXT
);
CREATE TABLE IF NOT EXISTS series (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS progress (
    book_id TEXT PRIMARY KEY,
    locator TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    book_id TEXT NOT NULL,
    started_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS bookmarks (
    id TEXT PRIMARY KEY,
    book_id TEXT NOT NULL,
    locator TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS annotations (
    id TEXT PRIMARY KEY,
    book_id TEXT NOT NULL,
    locator TEXT NOT NULL,
    body TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS servers (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    url TEXT NOT NULL,
    display_name TEXT NOT NULL,
    secret_ref TEXT
);
CREATE TABLE IF NOT EXISTS collections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_queue (
    id TEXT PRIMARY KEY,
    book_id TEXT NOT NULL,
    payload TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS download_queue (
    id TEXT PRIMARY KEY,
    url TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS search_index (
    book_id TEXT NOT NULL,
    chapter TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (book_id, chapter)
);
CREATE TABLE IF NOT EXISTS collection_items (
    collection_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    PRIMARY KEY (collection_id, item_id)
);
CREATE TABLE IF NOT EXISTS tags (
    book_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (book_id, tag)
);
CREATE TABLE IF NOT EXISTS ratings (
    book_id TEXT PRIMARY KEY,
    halves INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS smart_rules (
    collection_id TEXT PRIMARY KEY,
    field TEXT NOT NULL,
    expected TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS history (
    book_id TEXT PRIMARY KEY,
    opened_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS kavita_generation (
    server_id TEXT PRIMARY KEY,
    published INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS kavita_series (
    server_id TEXT NOT NULL,
    series_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    format TEXT NOT NULL,
    library_id INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, series_id)
);
CREATE TABLE IF NOT EXISTS kavita_library (
    server_id TEXT NOT NULL,
    library_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, library_id)
);
CREATE TABLE IF NOT EXISTS kavita_chapter (
    server_id TEXT NOT NULL,
    chapter_id INTEGER NOT NULL,
    series_id INTEGER NOT NULL,
    volume_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    pages INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, chapter_id)
);
CREATE TABLE IF NOT EXISTS kavita_home (
    server_id TEXT NOT NULL,
    stream_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    stream_type INTEGER NOT NULL,
    visible INTEGER NOT NULL,
    sort_order INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, stream_id)
);
CREATE TABLE IF NOT EXISTS kavita_collection (
    server_id TEXT NOT NULL,
    collection_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, collection_id)
);
CREATE TABLE IF NOT EXISTS kavita_reading_list (
    server_id TEXT NOT NULL,
    list_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, list_id)
);
CREATE TABLE IF NOT EXISTS kavita_filter (
    server_id TEXT NOT NULL,
    filter_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    filter TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, filter_id)
);
CREATE TABLE IF NOT EXISTS kavita_want (
    server_id TEXT NOT NULL,
    series_id INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, series_id)
);
CREATE TABLE IF NOT EXISTS kavita_bookmark (
    server_id TEXT NOT NULL,
    bookmark_id INTEGER NOT NULL,
    series_id INTEGER NOT NULL,
    chapter_id INTEGER NOT NULL,
    page INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, bookmark_id)
);
CREATE TABLE IF NOT EXISTS kavita_download (
    id TEXT PRIMARY KEY,
    server_id TEXT NOT NULL,
    url TEXT NOT NULL,
    status TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS kavita_progress (
    server_id TEXT NOT NULL,
    chapter_id INTEGER NOT NULL,
    locator TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, chapter_id)
);
CREATE TABLE IF NOT EXISTS kavita_shelf (
    server_id TEXT NOT NULL,
    shelf TEXT NOT NULL,
    series_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (server_id, shelf, series_id)
);
CREATE TABLE IF NOT EXISTS kavita_mark (
    server_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    scope_id INTEGER NOT NULL,
    series_id INTEGER NOT NULL,
    volume_id INTEGER NOT NULL,
    read INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, scope, scope_id)
);
CREATE TABLE IF NOT EXISTS kavita_outbox (
    server_id TEXT NOT NULL,
    chapter_id INTEGER NOT NULL,
    series_id INTEGER NOT NULL,
    volume_id INTEGER NOT NULL,
    library_id INTEGER NOT NULL,
    locator TEXT NOT NULL,
    complete INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, chapter_id)
);
";

pub(crate) fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(SCHEMA)?;
    ensure_version(conn, 1)?;
    ensure_item_column(conn, "label", "TEXT")?;
    ensure_item_column(conn, "size", "INTEGER")?;
    ensure_item_column(conn, "modified_ms", "INTEGER")?;
    ensure_item_column(conn, "series_guess", "TEXT")?;
    ensure_item_column(conn, "series", "TEXT")?;
    ensure_item_column(conn, "page_count", "INTEGER")?;
    ensure_item_column(conn, "image_child_count", "INTEGER")?;
    ensure_item_column(conn, "fingerprint", "TEXT")?;
    ensure_item_column(conn, "enriched_fingerprint", "TEXT")?;
    ensure_version(conn, 2)?;
    ensure_column(conn, "progress", "page", "INTEGER")?;
    ensure_column(conn, "progress", "chapter", "TEXT")?;
    ensure_column(conn, "progress", "complete", "INTEGER")?;
    ensure_column(conn, "sessions", "ended_at", "INTEGER")?;
    ensure_column(conn, "sessions", "pages_read", "INTEGER")?;
    ensure_column(conn, "bookmarks", "name", "TEXT")?;
    ensure_column(conn, "download_queue", "status", "TEXT")?;
    ensure_version(conn, 3)?;
    Ok(())
}

fn ensure_version(conn: &Connection, version: i64) -> Result<(), rusqlite::Error> {
    let present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
        [version],
        |row| row.get(0),
    )?;
    if present == 0 {
        let inserted = conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            [version],
        )?;
        if inserted != 1 {
            return Err(rusqlite::Error::StatementChangedRows(inserted));
        }
    }
    Ok(())
}

fn ensure_item_column(conn: &Connection, name: &str, decl: &str) -> Result<(), rusqlite::Error> {
    ensure_column(conn, "items", name, decl)
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    name: &str,
    decl: &str,
) -> Result<(), rusqlite::Error> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == name {
            return Ok(());
        }
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {name} {decl}"))?;
    Ok(())
}
