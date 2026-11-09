//! folio application library.
#![deny(missing_docs)]

#[cfg(any(test, feature = "desktop", feature = "mobile"))]
mod covers;
pub mod history;
pub mod kavita;
pub mod library;
#[cfg(any(test, feature = "desktop", feature = "mobile"))]
mod paginate;
pub mod reader;
pub mod search;
pub mod session;
pub mod settings;
pub mod shell;
mod state;
pub mod stats;

#[cfg(any(test, feature = "desktop", feature = "mobile"))]
mod chrome;
#[cfg(any(feature = "desktop", feature = "mobile"))]
mod ui;
#[cfg(any(feature = "desktop", feature = "mobile"))]
pub use ui::launch;

pub use state::{
    load_projection, AppError, LibraryState, PrefsState, Projection, ReaderState, ServerState,
};

use std::path::Path;

use folio_core::{ContentSource, SeekableSource, SliceSource};

/// Install tracing. Safe to call more than once.
pub fn prepare() {
    folio_core::init_tracing();
}

/// Touch every workspace crate so the workspace link is real.
pub fn linked_crates() -> &'static [&'static str] {
    let _source = ContentSource::path("book.epub");
    let _label = folio_library::filename_label("book.epub");
    let _extension = folio_formats::file_extension("book.epub");
    let pages = SliceSource::new(b"folio");
    if let Err(error) = pages.len() {
        eprintln!("slice length: {error}");
    }
    let _bind = folio_http::loopback_bind();
    let _data = folio_platform::data_dir(Path::new("."));
    let _mode = folio_reader::ReadingMode::Paginated;
    let _database = folio_storage::database_file_name();
    let connection = folio_server::ServerConnection::new(
        folio_server::ServerKind::Librarian,
        "http://127.0.0.1:9",
        "local",
        folio_server::AuthConfig::Anonymous,
    );
    if let Err(error) = connection.browse() {
        let _kind = error.kind;
    }
    let mut sync_queue = folio_sync::SyncQueue::new();
    let item = folio_sync::SyncQueueItem::progress("book-1", "epubcfi(/6/2)", 1);
    if let Err(error) = sync_queue.enqueue(item) {
        eprintln!("sync enqueue: {error}");
    }
    let _policy = folio_sync::SyncState::default().policy();
    &[
        "folio-core",
        "folio-formats",
        "folio-http",
        "folio-library",
        "folio-platform",
        "folio-reader",
        "folio-server",
        "folio-storage",
        "folio-sync",
        "folio-app",
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn server_and_sync_types_are_constructible() {
        let connection = folio_server::ServerConnection::new(
            folio_server::ServerKind::Librarian,
            "http://127.0.0.1:9",
            "local",
            folio_server::AuthConfig::LibrarianSession {
                secret_ref: "keyring:librarian:local".into(),
            },
        );
        assert!(matches!(
            connection.kind(),
            folio_server::ServerKind::Librarian
        ));
        let mut queue = folio_sync::SyncQueue::new();
        assert!(queue
            .enqueue(folio_sync::SyncQueueItem::progress(
                "book-1",
                "epubcfi(/6/2)",
                1
            ))
            .is_ok());
        assert_eq!(queue.len(), 1);
        let catalog = folio_server::parse_opds2(include_str!(
            "../../folio-server/tests/fixtures/opds/catalog.json"
        ))
        .expect("opds");
        let dir = tempfile::tempdir().expect("tempdir");
        let db = folio_storage::Database::open(&dir.path().join("folio.sqlite")).expect("open");
        for book in catalog.books() {
            folio_library::enqueue(&db, book.id(), book.download_url()).expect("queue");
        }
        let rows = folio_storage::downloads(&db).expect("downloads");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "queued");
        assert_eq!(rows[0].url, "http://127.0.0.1:9/dune.epub");
        assert!(db.catalog().expect("catalog").is_empty());
    }
}
