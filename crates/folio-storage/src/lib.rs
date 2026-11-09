//! SQLite is the source of truth. One background writer owns the connection.
#![deny(missing_docs)]

mod backup;
mod bookmark;
mod cache;
mod catalog;
mod collection;
mod kavita;
mod library;
mod migrate;
mod notes;
mod progress;
mod queue;
mod session;
mod stats;

pub use backup::{
    export_backup, flag_or_off, import_backup, is_copied_file, write_log, Backup, SCHEMA_VERSION,
};
pub use bookmark::Bookmark;
pub use catalog::CatalogItem;
pub use collection::{
    add_collection_item, add_tag, clear_history, clear_history_one, collection_items,
    create_collection, history, open_history, rating, remove_collection_item, set_rating,
    set_smart_rule, smart_rule, tags,
};
pub use kavita::{
    acknowledge_kavita_mark, acknowledge_kavita_progress, apply_kavita_pull, claim_kavita_download,
    enqueue_kavita_download, enqueue_kavita_mark, enqueue_kavita_progress, forget_kavita_server,
    kavita_download_status, kavita_downloads, kavita_downloads_by_status, kavita_marks,
    kavita_outbox, kavita_pending, publish_kavita_mirror, replace_kavita_chapters,
    reset_kavita_server, settle_kavita_download, visible_kavita_chapters,
    visible_kavita_collections, visible_kavita_filters, visible_kavita_mirror,
    visible_kavita_reading_lists, visible_kavita_series, KavitaBookmarkRow, KavitaChapterRow,
    KavitaDownloadRow, KavitaHomeRow, KavitaKeep, KavitaLibraryRow, KavitaMarkRow, KavitaMirror,
    KavitaNamedRow, KavitaOutboxRow, KavitaSeriesRow, KavitaVisible,
};
pub use notes::StoredAnnotation;
pub use progress::{
    is_complete, land_before_stop, PendingReading, ProgressDebounce, ReadingProgress,
};
pub use queue::{
    downloads, enqueue_download, remove_server, servers, set_download_status, swap_server,
    upsert_server, DownloadRow, StoredServer,
};
pub use session::ReadingSession;
pub use stats::{summarise, ReadingStats, DAY_MS};

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rusqlite::Connection;
use thiserror::Error;

pub use cache::{CoverCache, PageCache};

/// File name of the library database, relative to the app data directory.
pub const DATABASE_FILE_NAME: &str = "folio.sqlite";

/// [`DATABASE_FILE_NAME`] as a function, for callers that pass function pointers.
pub fn database_file_name() -> &'static str {
    DATABASE_FILE_NAME
}

/// Failure from the database or a cache beside it.
#[derive(Debug, Error)]
pub enum StorageError {
    /// SQLite returned an error.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The writer thread is gone.
    #[error("storage writer stopped")]
    Closed,
    /// The system clock is before the Unix epoch, or the instant does not fit.
    #[error("clock error: {0}")]
    Clock(String),
    /// A book id or locator was empty.
    #[error("book id and locator must not be empty")]
    EmptyProgress,
    /// A setting key or value was empty.
    #[error("setting key and value must not be empty")]
    EmptySetting,
    /// An id, book id, or locator was empty.
    #[error("id must not be empty")]
    EmptyId,
    /// A cache directory could not be placed beside the database.
    #[error("database path has no parent directory")]
    Path,
    /// The page cache was asked to store nothing.
    #[error("page cache limit must be at least one entry")]
    CacheLimit,
    /// Cover bytes were not a WebP image.
    #[error("cover bytes are not WebP")]
    NotWebp,
    /// A cache mutex was poisoned.
    #[error("cache lock poisoned")]
    LockPoisoned,
    /// Filesystem failure while writing a cache.
    #[error("cache i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// An update or delete did not match a row.
    #[error("the row was not found")]
    Missing,
    /// A backup schema version is not one this build can import.
    #[error("backup schema is not supported")]
    Schema,
    /// A download URL was already queued.
    #[error("download url is already queued")]
    Duplicate,
    /// Another row already has the id a row was being moved to.
    #[error("another row already has that id")]
    Taken,
    /// A rating was outside one half-star to five stars.
    #[error("rating must be from one half-star to five stars")]
    Rating,
}

enum Job {
    SetProgress {
        book_id: String,
        locator: String,
        reply: Sender<Result<(), StorageError>>,
    },
    Progress {
        book_id: String,
        reply: Sender<Result<Option<String>, StorageError>>,
    },
    LatestLocator {
        reply: Sender<Result<Option<String>, StorageError>>,
    },
    ItemIds {
        reply: Sender<Result<Vec<String>, StorageError>>,
    },
    Settings {
        reply: Sender<Result<Vec<(String, String)>, StorageError>>,
    },
    SetSetting {
        key: String,
        value: String,
        reply: Sender<Result<(), StorageError>>,
    },
    ServerIds {
        reply: Sender<Result<Vec<String>, StorageError>>,
    },
    UpsertItem {
        // Box: CatalogItem is much larger than the other jobs.
        item: Box<CatalogItem>,
        reply: Sender<Result<(), StorageError>>,
    },
    Catalog {
        reply: Sender<Result<Vec<CatalogItem>, StorageError>>,
    },
    DeleteItem {
        id: String,
        reply: Sender<Result<(), StorageError>>,
    },
    SetReading {
        book_id: String,
        locator: String,
        page: i64,
        chapter: String,
        complete: i64,
        updated_at: i64,
        reply: Sender<Result<(), StorageError>>,
    },
    Reading {
        book_id: String,
        reply: Sender<Result<Option<ReadingProgress>, StorageError>>,
    },
    StartSession {
        id: String,
        book_id: String,
        started_at: i64,
        reply: Sender<Result<(), StorageError>>,
    },
    FinishSession {
        id: String,
        ended_at: i64,
        pages_read: i64,
        reply: Sender<Result<(), StorageError>>,
    },
    Sessions {
        reply: Sender<Result<Vec<ReadingSession>, StorageError>>,
    },
    InsertBookmark {
        // Box: Bookmark is larger than the shutdown job.
        bookmark: Box<Bookmark>,
        reply: Sender<Result<(), StorageError>>,
    },
    RenameBookmark {
        id: String,
        name: String,
        reply: Sender<Result<(), StorageError>>,
    },
    DeleteBookmark {
        id: String,
        reply: Sender<Result<(), StorageError>>,
    },
    Bookmarks {
        book_id: String,
        reply: Sender<Result<Vec<Bookmark>, StorageError>>,
    },
    UpsertAnnotation {
        annotation: Box<StoredAnnotation>,
        reply: Sender<Result<(), StorageError>>,
    },
    Annotations {
        book_id: String,
        reply: Sender<Result<Vec<StoredAnnotation>, StorageError>>,
    },
    DeleteAnnotation {
        id: String,
        reply: Sender<Result<(), StorageError>>,
    },
    ReplaceSearch {
        book_id: String,
        chapters: Vec<(String, String)>,
        reply: Sender<Result<(), StorageError>>,
    },
    SearchChapters {
        book_id: String,
        reply: Sender<Result<Vec<(String, String)>, StorageError>>,
    },
    Work(Box<dyn FnOnce(&mut Connection) + Send>),
    Shutdown,
}

/// Open database. Methods send work to the single writer thread.
#[derive(Debug)]
pub struct Database {
    path: PathBuf,
    tx: Sender<Job>,
    thread: Option<JoinHandle<()>>,
}

impl Database {
    /// Open or create `path` and run migrations on the writer thread.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be opened or migrated.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let path = path.to_path_buf();
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        // clone: the writer thread and this handle each own the database path
        let writer_path = path.clone();
        let thread = thread::spawn(move || writer(writer_path, rx, ready_tx));
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                path,
                tx,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                join_writer(thread);
                Err(error)
            }
            Err(_) => {
                join_writer(thread);
                Err(StorageError::Closed)
            }
        }
    }

    /// Insert or replace the locator for `book_id`. The write is committed before this returns.
    ///
    /// # Errors
    ///
    /// Returns an error when the id or locator is empty, or the writer cannot commit.
    pub fn set_progress(&self, book_id: &str, locator: &str) -> Result<(), StorageError> {
        if book_id.is_empty() || locator.is_empty() {
            return Err(StorageError::EmptyProgress);
        }
        self.roundtrip(|reply| Job::SetProgress {
            // to_owned: the writer thread stores the row after this call returns
            book_id: book_id.to_owned(),
            locator: locator.to_owned(),
            reply,
        })
    }

    /// Locator saved for `book_id`, if any.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn progress(&self, book_id: &str) -> Result<Option<String>, StorageError> {
        self.roundtrip(|reply| Job::Progress {
            // to_owned: the writer thread reads the row on its own stack
            book_id: book_id.to_owned(),
            reply,
        })
    }

    /// Locator with the newest timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn latest_locator(&self) -> Result<Option<String>, StorageError> {
        self.roundtrip(|reply| Job::LatestLocator { reply })
    }

    /// Item ids, in id order.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn item_ids(&self) -> Result<Vec<String>, StorageError> {
        self.roundtrip(|reply| Job::ItemIds { reply })
    }

    /// Setting rows, in key order.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn settings(&self) -> Result<Vec<(String, String)>, StorageError> {
        self.roundtrip(|reply| Job::Settings { reply })
    }

    /// Insert or replace one setting. The write is committed before this returns.
    ///
    /// # Errors
    ///
    /// Returns an error when the key or value is empty, or the writer cannot commit.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StorageError> {
        if key.is_empty() || value.is_empty() {
            return Err(StorageError::EmptySetting);
        }
        self.roundtrip(|reply| Job::SetSetting {
            // to_owned: the writer thread stores the row after this call returns
            key: key.to_owned(),
            value: value.to_owned(),
            reply,
        })
    }

    /// Server ids, in id order.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn server_ids(&self) -> Result<Vec<String>, StorageError> {
        self.roundtrip(|reply| Job::ServerIds { reply })
    }

    /// Insert or replace one catalog row. The write is committed before this returns.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot commit.
    pub fn upsert_item(&self, item: CatalogItem) -> Result<(), StorageError> {
        self.roundtrip(|reply| Job::UpsertItem {
            item: Box::new(item),
            reply,
        })
    }

    /// Catalog rows, in id order.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn catalog(&self) -> Result<Vec<CatalogItem>, StorageError> {
        self.roundtrip(|reply| Job::Catalog { reply })
    }

    /// Remove one catalog row. Progress for that id is left in place.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot commit.
    pub fn delete_item(&self, id: &str) -> Result<(), StorageError> {
        self.roundtrip(|reply| Job::DeleteItem {
            // to_owned: the writer thread deletes the row after this call returns
            id: id.to_owned(),
            reply,
        })
    }

    /// Insert or replace page, chapter, completion, and locator.
    ///
    /// # Errors
    ///
    /// Returns an error when `book_id` or `locator` is empty, or the writer cannot commit.
    pub fn set_reading(
        &self,
        book_id: &str,
        locator: &str,
        page: u32,
        chapter: &str,
        complete: bool,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        if book_id.is_empty() || locator.is_empty() {
            return Err(StorageError::EmptyProgress);
        }
        self.roundtrip(|reply| Job::SetReading {
            book_id: book_id.to_owned(),
            locator: locator.to_owned(),
            page: i64::from(page),
            chapter: chapter.to_owned(),
            complete: if complete { 1 } else { 0 },
            updated_at,
            reply,
        })
    }

    /// Page, chapter, and completion saved for `book_id`.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn reading(&self, book_id: &str) -> Result<Option<ReadingProgress>, StorageError> {
        self.roundtrip(|reply| Job::Reading {
            book_id: book_id.to_owned(),
            reply,
        })
    }

    /// Open a session. The write is committed before this returns.
    ///
    /// # Errors
    ///
    /// Returns an error when an id is empty or the writer cannot commit.
    pub fn start_session(
        &self,
        id: &str,
        book_id: &str,
        started_at: i64,
    ) -> Result<(), StorageError> {
        if id.is_empty() || book_id.is_empty() {
            return Err(StorageError::EmptyId);
        }
        self.roundtrip(|reply| Job::StartSession {
            id: id.to_owned(),
            book_id: book_id.to_owned(),
            started_at,
            reply,
        })
    }

    /// Close a session.
    ///
    /// # Errors
    ///
    /// Returns an error when the session does not exist or the writer cannot commit.
    pub fn finish_session(
        &self,
        id: &str,
        ended_at: i64,
        pages_read: i64,
    ) -> Result<(), StorageError> {
        if id.is_empty() {
            return Err(StorageError::EmptyId);
        }
        self.roundtrip(|reply| Job::FinishSession {
            id: id.to_owned(),
            ended_at,
            pages_read,
            reply,
        })
    }

    /// Sessions in start order.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn sessions(&self) -> Result<Vec<ReadingSession>, StorageError> {
        self.roundtrip(|reply| Job::Sessions { reply })
    }

    /// Insert a bookmark.
    ///
    /// # Errors
    ///
    /// Returns an error when an id or the locator is empty, or the writer cannot commit.
    pub fn insert_bookmark(&self, bookmark: Bookmark) -> Result<(), StorageError> {
        if bookmark.id.is_empty() || bookmark.book_id.is_empty() || bookmark.locator.is_empty() {
            return Err(StorageError::EmptyId);
        }
        self.roundtrip(|reply| Job::InsertBookmark {
            bookmark: Box::new(bookmark),
            reply,
        })
    }

    /// Rename a bookmark.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is empty, the row is missing, or the writer cannot commit.
    pub fn rename_bookmark(&self, id: &str, name: &str) -> Result<(), StorageError> {
        if id.is_empty() || name.is_empty() {
            return Err(StorageError::EmptyId);
        }
        self.roundtrip(|reply| Job::RenameBookmark {
            id: id.to_owned(),
            name: name.to_owned(),
            reply,
        })
    }

    /// Delete a bookmark.
    ///
    /// # Errors
    ///
    /// Returns an error when the row is missing or the writer cannot commit.
    pub fn delete_bookmark(&self, id: &str) -> Result<(), StorageError> {
        if id.is_empty() {
            return Err(StorageError::EmptyId);
        }
        self.roundtrip(|reply| Job::DeleteBookmark {
            id: id.to_owned(),
            reply,
        })
    }

    /// Bookmarks for one book, in id order.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn bookmarks(&self, book_id: &str) -> Result<Vec<Bookmark>, StorageError> {
        self.roundtrip(|reply| Job::Bookmarks {
            book_id: book_id.to_owned(),
            reply,
        })
    }

    /// Insert or replace an annotation.
    ///
    /// # Errors
    ///
    /// Returns an error when an id or the locator is empty, or the writer cannot commit.
    pub fn upsert_annotation(&self, annotation: StoredAnnotation) -> Result<(), StorageError> {
        if annotation.id.is_empty()
            || annotation.book_id.is_empty()
            || annotation.locator.is_empty()
        {
            return Err(StorageError::EmptyId);
        }
        self.roundtrip(|reply| Job::UpsertAnnotation {
            annotation: Box::new(annotation),
            reply,
        })
    }

    /// Annotations for one book, in id order.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn annotations(&self, book_id: &str) -> Result<Vec<StoredAnnotation>, StorageError> {
        self.roundtrip(|reply| Job::Annotations {
            book_id: book_id.to_owned(),
            reply,
        })
    }

    /// Delete an annotation.
    ///
    /// # Errors
    ///
    /// Returns an error when the row is missing or the writer cannot commit.
    pub fn delete_annotation(&self, id: &str) -> Result<(), StorageError> {
        if id.is_empty() {
            return Err(StorageError::EmptyId);
        }
        self.roundtrip(|reply| Job::DeleteAnnotation {
            id: id.to_owned(),
            reply,
        })
    }

    /// Replace the search rows for one book. Other books are left alone.
    ///
    /// # Errors
    ///
    /// Returns an error when `book_id` is empty or the writer cannot commit.
    pub fn replace_search(
        &self,
        book_id: &str,
        chapters: Vec<(String, String)>,
    ) -> Result<(), StorageError> {
        if book_id.is_empty() {
            return Err(StorageError::EmptyId);
        }
        self.roundtrip(|reply| Job::ReplaceSearch {
            book_id: book_id.to_owned(),
            chapters,
            reply,
        })
    }

    /// Chapter text stored for one book's search index.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer cannot read.
    pub fn search_chapters(&self, book_id: &str) -> Result<Vec<(String, String)>, StorageError> {
        self.roundtrip(|reply| Job::SearchChapters {
            book_id: book_id.to_owned(),
            reply,
        })
    }

    /// WebP cover cache directory beside this database.
    ///
    /// # Errors
    ///
    /// Returns an error when the cache directory cannot be created.
    pub fn cover_cache(&self) -> Result<CoverCache, StorageError> {
        CoverCache::open(&self.path)
    }

    /// Content-addressed page cache beside this database.
    ///
    /// # Errors
    ///
    /// Returns an error when the cache directory cannot be created or the limit is zero.
    pub fn page_cache(&self, max_entries: usize) -> Result<PageCache, StorageError> {
        PageCache::open(&self.path, max_entries)
    }

    fn roundtrip<T>(
        &self,
        make: impl FnOnce(Sender<Result<T, StorageError>>) -> Job,
    ) -> Result<T, StorageError>
    where
        T: Send + 'static,
    {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(make(reply_tx))
            .map_err(|_| StorageError::Closed)?;
        reply_rx.recv().map_err(|_| StorageError::Closed)?
    }

    pub(crate) fn work<T: Send + 'static>(
        &self,
        run: impl FnOnce(&mut Connection) -> Result<T, StorageError> + Send + 'static,
    ) -> Result<T, StorageError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(Job::Work(Box::new(move |conn| {
                let result = run(conn);
                if reply_tx.send(result).is_err() {
                    eprintln!("storage caller dropped before the reply");
                }
            })))
            .map_err(|_| StorageError::Closed)?;
        reply_rx.recv().map_err(|_| StorageError::Closed)?
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        if self.tx.send(Job::Shutdown).is_err() {
            eprintln!("storage writer already stopped");
        }
        if let Some(thread) = self.thread.take() {
            join_writer(thread);
        }
    }
}

fn join_writer(thread: JoinHandle<()>) {
    if let Err(panic) = thread.join() {
        eprintln!("storage writer panicked: {panic:?}");
    }
}

fn writer(path: PathBuf, rx: Receiver<Job>, ready: Sender<Result<(), StorageError>>) {
    let mut conn = match open_connection(&path) {
        Ok(conn) => conn,
        Err(error) => {
            if ready.send(Err(error)).is_err() {
                eprintln!("storage open was cancelled before the error was read");
            }
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        eprintln!("storage open was cancelled");
        return;
    }
    while let Ok(job) = rx.recv() {
        match job {
            Job::Shutdown => break,
            Job::SetProgress {
                book_id,
                locator,
                reply,
            } => answer(&reply, progress::set_progress(&conn, &book_id, &locator)),
            Job::Progress { book_id, reply } => {
                answer(&reply, progress::progress(&conn, &book_id));
            }
            Job::LatestLocator { reply } => answer(&reply, progress::latest_locator(&conn)),
            Job::ItemIds { reply } => answer(&reply, library::item_ids(&conn)),
            Job::Settings { reply } => answer(&reply, library::settings(&conn)),
            Job::SetSetting { key, value, reply } => {
                answer(&reply, library::set_setting(&conn, &key, &value));
            }
            Job::ServerIds { reply } => answer(&reply, library::server_ids(&conn)),
            Job::UpsertItem { item, reply } => answer(&reply, catalog::upsert(&conn, &item)),
            Job::Catalog { reply } => answer(&reply, catalog::items(&conn)),
            Job::DeleteItem { id, reply } => answer(&reply, catalog::delete_item(&conn, &id)),
            Job::SetReading {
                book_id,
                locator,
                page,
                chapter,
                complete,
                updated_at,
                reply,
            } => answer(
                &reply,
                progress::set_reading(
                    &conn, &book_id, &locator, page, &chapter, complete, updated_at,
                ),
            ),
            Job::Reading { book_id, reply } => answer(&reply, progress::reading(&conn, &book_id)),
            Job::StartSession {
                id,
                book_id,
                started_at,
                reply,
            } => answer(&reply, session::start(&conn, &id, &book_id, started_at)),
            Job::FinishSession {
                id,
                ended_at,
                pages_read,
                reply,
            } => answer(&reply, session::finish(&conn, &id, ended_at, pages_read)),
            Job::Sessions { reply } => answer(&reply, session::list(&conn)),
            Job::InsertBookmark { bookmark, reply } => {
                answer(&reply, bookmark::insert(&conn, &bookmark));
            }
            Job::RenameBookmark { id, name, reply } => {
                answer(&reply, bookmark::rename(&conn, &id, &name));
            }
            Job::DeleteBookmark { id, reply } => answer(&reply, bookmark::delete(&conn, &id)),
            Job::Bookmarks { book_id, reply } => answer(&reply, bookmark::list(&conn, &book_id)),
            Job::UpsertAnnotation { annotation, reply } => {
                answer(&reply, notes::upsert_annotation(&conn, &annotation));
            }
            Job::Annotations { book_id, reply } => {
                answer(&reply, notes::annotations(&conn, &book_id));
            }
            Job::DeleteAnnotation { id, reply } => {
                answer(&reply, notes::delete_annotation(&conn, &id));
            }
            Job::ReplaceSearch {
                book_id,
                chapters,
                reply,
            } => answer(
                &reply,
                notes::replace_search(&mut conn, &book_id, &chapters),
            ),
            Job::SearchChapters { book_id, reply } => {
                answer(&reply, notes::search_chapters(&conn, &book_id));
            }
            Job::Work(run) => run(&mut conn),
        }
    }
}

fn open_connection(path: &Path) -> Result<Connection, StorageError> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    migrate::migrate(&conn)?;
    Ok(conn)
}

fn answer<T>(reply: &Sender<Result<T, StorageError>>, result: Result<T, StorageError>) {
    if reply.send(result).is_err() {
        eprintln!("storage caller dropped before the reply");
    }
}
