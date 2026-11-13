//! File open, identity scan, and storage writes. This module does not render.
//!
//! Chapter markup stays here. The screen receives text blocks, never a HTML string.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use folio_core::{FileType, SeekableSource};
use folio_formats::{
    open_cb7, open_cbr, open_cbt, open_cbz, open_epub, open_image_folder, open_mobi,
    pdf_page_count, render_pdf_png, ImageEntry,
};
use folio_library::{scan_listing, ScanDepth, ScanOutcome, ScanRequest};
use folio_platform::list_directory;
use folio_server::{progress_write_supported, AuthConfig, ServerConnection, ServerKind};
use folio_storage::{
    summarise, Bookmark, CatalogItem, Database, ReadingSession, ReadingStats, StoredServer,
};
use folio_sync::{Adapter, ReplayStatus, SyncQueue, SyncQueueItem};

use crate::kavita::MirrorStep;
use crate::shell::ShelfProgress;

static SESSION_SEQ: AtomicU64 = AtomicU64::new(1);

/// Folders under the device library parent, in scan order.
pub const LIBRARY_FOLDERS: [&str; 5] = ["Books", "Articles", "Kids", "Download", "Documents"];

/// Why a library or reader operation stopped.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// A file or directory could not be read.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The catalog database rejected a read or a write.
    #[error(transparent)]
    Storage(#[from] folio_storage::StorageError),
    /// The identity scan could not be stored.
    #[error(transparent)]
    Scan(#[from] folio_library::ScanError),
    /// A book parser rejected the file.
    #[error(transparent)]
    Format(#[from] folio_formats::FormatError),
    /// A progress row could not be queued.
    #[error(transparent)]
    Sync(#[from] folio_sync::SyncError),
    /// `relative` left the library root.
    #[error("a catalog path escapes the library root")]
    Escape,
    /// The clock is before the Unix epoch or does not fit in `i64`.
    #[error("the clock could not be read")]
    Clock,
    /// The file type is not opened by this shell.
    #[error("that book format is not supported")]
    Unsupported,
    /// A chapter or page index did not fit in `u32`.
    #[error("a page index did not fit")]
    Number,
    /// The server form was empty or the kind is unknown.
    #[error("the server record is incomplete")]
    Server,
    /// The process-wide database is not open, or its lock was poisoned.
    #[error("the library database is not open")]
    Closed,
    /// The credential file could not be used.
    #[error(transparent)]
    Secret(#[from] folio_platform::SecretError),
    /// A Kavita request failed.
    #[error(transparent)]
    Remote(#[from] folio_server::ServerError),
    /// A Kavita request made during a sync failed.
    #[error("Kavita {step}: {error}")]
    Mirror {
        /// The request that failed.
        step: MirrorStep,
        /// How it failed.
        error: folio_server::ServerError,
    },
    /// A Kavita download failed.
    #[error(transparent)]
    Download(#[from] folio_library::KavitaDownloadError),
    /// The server was edited or removed while its replies were read, so they were not stored.
    #[error("the server was changed while Folio read it")]
    Changed,
    /// This series is not in the Auth Key's Kavita list.
    #[error("That series is not in the library.")]
    Hidden,
    /// An edit would give a server the name another saved server has.
    #[error("a server with that name is already saved")]
    NameTaken,
}

/// A file the parsers can seek. The bytes stay on disk.
#[derive(Debug)]
pub struct FileSource {
    file: File,
    length: u64,
}

impl FileSource {
    /// Open `path` for random reads.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be opened or its length cannot be read.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        Ok(Self { file, length })
    }
}

impl SeekableSource for FileSource {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset > self.length {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "read offset is past the end",
            ));
        }
        if offset == self.length || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.length - offset;
        let want = u64::try_from(buf.len()).unwrap_or(remaining).min(remaining);
        let want = usize::try_from(want)
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read(&mut buf[..want])
    }

    fn len(&self) -> io::Result<u64> {
        Ok(self.length)
    }
}

/// One block of chapter text. The string is decoded text, not markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextBlock {
    /// A heading. `level` is 1 through 6.
    Heading {
        /// Heading rank.
        level: u8,
        /// Visible text.
        text: String,
    },
    /// A paragraph or list item.
    Paragraph {
        /// Visible text.
        text: String,
    },
}

/// Turn chapter markup into text blocks.
///
/// Script, style, and head elements are dropped, including their text.
/// Processing instructions and declarations are dropped. Character references
/// are decoded. The result contains no tags.
pub fn chapter_blocks(html: &str) -> Vec<TextBlock> {
    let bytes = html.as_bytes();
    let mut index = 0;
    let mut blocks = Vec::new();
    let mut buffer = String::new();
    let mut current: Option<OpenBlock> = None;
    while index < bytes.len() {
        if bytes[index] == b'<' {
            if bytes.get(index.saturating_add(1)) == Some(&b'?') {
                index = find_slice(bytes, index, b"?>")
                    .map(|position| position.saturating_add(2))
                    .unwrap_or(bytes.len());
                continue;
            }
            if let Some(tag) = parse_tag(bytes, index) {
                index = tag.next;
                if tag.comment {
                    continue;
                }
                if !tag.closing && matches!(tag.name.as_str(), "script" | "style" | "head") {
                    index = skip_until_close(bytes, index, &tag.name);
                    continue;
                }
                if tag.closing {
                    if let Some(open) = current.as_ref() {
                        if open.tag == tag.name {
                            push_block(&mut blocks, open.kind, &buffer);
                            buffer.clear();
                            current = None;
                        }
                    }
                    continue;
                }
                if let Some(kind) = block_kind(&tag.name) {
                    flush_loose(&mut blocks, &mut buffer, current.take());
                    current = Some(OpenBlock {
                        kind,
                        tag: tag.name,
                    });
                    continue;
                }
                if matches!(tag.name.as_str(), "br" | "hr") {
                    buffer.push(' ');
                }
                continue;
            }
        }
        if bytes[index] == b'&' {
            if let Some((character, next)) = decode_entity(bytes, index) {
                buffer.push(character);
                index = next;
                continue;
            }
        }
        push_utf8(&mut buffer, bytes, &mut index);
    }
    flush_loose(&mut blocks, &mut buffer, current);
    blocks
}

/// Standard base64, with padding.
pub fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    let mut index = 0;
    while index + 3 <= bytes.len() {
        push_quantum(
            &mut out,
            TABLE,
            (u32::from(bytes[index]) << 16)
                | (u32::from(bytes[index + 1]) << 8)
                | u32::from(bytes[index + 2]),
            4,
        );
        index = index.saturating_add(3);
    }
    let rest = bytes.len() - index;
    if rest == 1 {
        push_quantum(&mut out, TABLE, u32::from(bytes[index]) << 16, 2);
        out.push('=');
        out.push('=');
    } else if rest == 2 {
        let value = (u32::from(bytes[index]) << 16) | (u32::from(bytes[index + 1]) << 8);
        push_quantum(&mut out, TABLE, value, 3);
        out.push('=');
    }
    out
}

/// MIME type for a page file name. Unknown names are `application/octet-stream`.
pub fn page_media_type(name: &str) -> &'static str {
    let extension = name.rsplit('.').next().unwrap_or("");
    if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
        "image/jpeg"
    } else if extension.eq_ignore_ascii_case("png") {
        "image/png"
    } else if extension.eq_ignore_ascii_case("webp") {
        "image/webp"
    } else if extension.eq_ignore_ascii_case("gif") {
        "image/gif"
    } else if extension.eq_ignore_ascii_case("avif") {
        "image/avif"
    } else {
        "application/octet-stream"
    }
}

/// `data:` URL for one page. Callers pass the visible page only.
pub fn page_data_url(name: &str, bytes: &[u8]) -> String {
    let mut url = String::from("data:");
    url.push_str(page_media_type(name));
    url.push_str(";base64,");
    url.push_str(&encode_base64(bytes));
    url
}

/// Device library parent. Android shared storage lives here.
pub fn library_parent() -> PathBuf {
    PathBuf::from("/storage/emulated/0")
}

/// Application files directory, outside the scanned folders.
///
/// On Android this is the package files dir. On the host it is `./folio`.
pub fn app_files_dir() -> PathBuf {
    let primary = Path::new("/data/user/0/dev.folio.reader/files");
    let legacy = Path::new("/data/data/dev.folio.reader/files");
    if cfg!(target_os = "android") {
        if primary.is_dir() || !legacy.is_dir() {
            return primary.to_path_buf();
        }
        return legacy.to_path_buf();
    }
    if primary.is_dir() {
        return primary.to_path_buf();
    }
    folio_platform::data_dir(Path::new("."))
}

/// Cover directory beside the database.
pub fn cover_dir(files_dir: &Path) -> PathBuf {
    files_dir.join("covers")
}

/// Create the files directory and open `folio.sqlite` inside it.
///
/// # Errors
///
/// Returns an error when the directory cannot be created or the database cannot be opened.
pub fn open_database(files_dir: &Path) -> Result<Database, SessionError> {
    fs::create_dir_all(files_dir)?;
    fs::create_dir_all(cover_dir(files_dir))?;
    Ok(Database::open(
        &files_dir.join(folio_storage::database_file_name()),
    )?)
}

/// Join `relative` onto `library` and reject paths that leave it.
///
/// # Errors
///
/// Returns [`SessionError::Escape`] when `relative` is absolute, empty, or contains `..`.
/// An existing symlink that resolves outside `library` is the same error.
pub fn book_path(library: &Path, relative: &str) -> Result<PathBuf, SessionError> {
    let joined = lexical_join(library, relative)?;
    match library.canonicalize() {
        Ok(root) => contained(&root, &joined),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(joined),
        Err(error) => Err(error.into()),
    }
}

/// One identity scan of the library folders that exist under `parent`.
///
/// Listings are prefixed with the folder name and stored in one call, so a
/// second folder does not retire the first. Pass 2 does not run. A folder
/// that cannot be listed keeps previously stored rows.
///
/// # Errors
///
/// Returns an error when the catalog write fails. An unreadable folder is
/// reported on [`LibraryScan::unreadable`] instead.
pub fn scan_libraries(
    parent: &Path,
    db: &Database,
    cover_dir: &Path,
) -> Result<LibraryScan, SessionError> {
    let mut entries = Vec::new();
    let mut unreadable = Vec::new();
    let mut complete = true;
    for folder in LIBRARY_FOLDERS {
        let dir = parent.join(folder);
        match fs::metadata(&dir) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => continue,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                complete = false;
                unreadable.push(format!("{folder}: {error}"));
                continue;
            }
        }
        match list_directory(&dir) {
            Ok(listed) => {
                for mut entry in listed {
                    entry.relative = format!("{folder}/{}", entry.relative);
                    entries.push(entry);
                }
            }
            Err(error) => {
                complete = false;
                unreadable.push(format!("{folder}: {}", error_chain(&error)));
            }
        }
    }
    let cancel = AtomicBool::new(false);
    let request = ScanRequest {
        entries: &entries,
        complete,
        cover_dir,
        cancel: &cancel,
        depth: ScanDepth::Identity,
    };
    let mut sniff = |relative: &str| -> Option<Vec<u8>> {
        let path = book_path(parent, relative).ok()?;
        let mut source = FileSource::open(&path).ok()?;
        let mut buf = [0u8; 512];
        let count = source.read_at(0, &mut buf).ok()?;
        Some(buf[..count].to_vec())
    };
    let mut open = |_relative: &str, _kind: FileType| {
        Err(folio_library::ScanError::Format(
            folio_formats::FormatError::Invalid { kind: "identity" },
        ))
    };
    let outcome = scan_listing(db, &request, &mut sniff, &mut open)?;
    Ok(LibraryScan {
        outcome,
        unreadable,
    })
}

/// Result of [`scan_libraries`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryScan {
    /// Rows stored by the identity scan.
    pub outcome: ScanOutcome,
    /// Folders that exist but could not be listed, with the platform error.
    pub unreadable: Vec<String>,
}

/// A book opened for the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenedBook {
    /// EPUB chapters as text blocks. A picture stands in when a chapter has no text.
    Text {
        /// Title from the package, or the file name.
        title: String,
        /// Spine order.
        chapters: Vec<Vec<TextBlock>>,
        /// Picture for a chapter whose text is empty.
        pictures: Vec<Option<PageBytes>>,
    },
    /// Comic or image-folder pages. Encode one page at a time.
    Pages {
        /// Series or file name.
        title: String,
        /// Page bytes, in reading order.
        pages: Arc<Vec<PageBytes>>,
    },
    /// A PDF. One page is drawn when the reader asks for it.
    Pdf {
        /// File name, until the document has a title.
        title: String,
        /// The PDF file. Rendering parses it again for the visible page.
        bytes: Arc<Vec<u8>>,
        /// Page count from the parser.
        page_count: usize,
    },
}

/// One comic or image-folder page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBytes {
    /// File name inside the book, used for the media type.
    pub name: String,
    /// Encoded image bytes.
    pub bytes: Vec<u8>,
}

impl OpenedBook {
    /// Title shown above the page.
    pub fn title(&self) -> &str {
        match self {
            Self::Text { title, .. } | Self::Pages { title, .. } | Self::Pdf { title, .. } => title,
        }
    }

    /// Chapter or page count.
    pub fn len(&self) -> usize {
        match self {
            Self::Text { chapters, .. } => chapters.len(),
            Self::Pages { pages, .. } => pages.len(),
            Self::Pdf { page_count, .. } => *page_count,
        }
    }

    /// Whether there is no chapter and no page.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Text of chapter `index`, or an empty slice when this is not text.
    pub fn blocks(&self, index: usize) -> &[TextBlock] {
        match self {
            Self::Text { chapters, .. } => chapters.get(index).map(Vec::as_slice).unwrap_or(&[]),
            Self::Pages { .. } | Self::Pdf { .. } => &[],
        }
    }

    /// PNG of PDF page `index`.
    ///
    /// # Errors
    ///
    /// Returns an error when this is not a PDF, the index is past the end, or
    /// the page cannot be drawn.
    pub fn pdf_png(&self, index: usize) -> Result<Vec<u8>, SessionError> {
        let Self::Pdf {
            bytes, page_count, ..
        } = self
        else {
            return Err(SessionError::Unsupported);
        };
        if index >= *page_count {
            return Err(SessionError::Number);
        }
        Ok(render_pdf_png(bytes, index)?)
    }
}

/// Open one catalog id under `library`.
///
/// EPUB and MOBI return text. PDF returns the file so one page can be drawn.
/// CBZ, CB7, CBT, CBR, and image folders return page images.
///
/// # Errors
///
/// Returns an error when the path escapes `library` or the file cannot be parsed.
pub fn open_at(
    library: &Path,
    relative: &str,
    file_type: &str,
) -> Result<OpenedBook, SessionError> {
    let path = book_path(library, relative)?;
    match file_type {
        "pdf" => open_pdf_file(&path, relative),
        "cbr" => open_comic_file(&path, relative, file_type),
        "epub" => open_epub_file(&path, relative),
        "mobi" => open_mobi_file(&path, relative),
        "cbz" | "cb7" | "cbt" => open_comic_file(&path, relative, file_type),
        "image-folder" => open_image_dir(&path, relative),
        _ => Err(SessionError::Unsupported),
    }
}

/// Chapter index stored as `chapter:{n}` or `page:{n}`.
pub fn place_index(locator: &str) -> Option<usize> {
    let rest = locator
        .strip_prefix("chapter:")
        .or_else(|| locator.strip_prefix("page:"))?;
    let value = rest.parse::<u32>().ok()?;
    usize::try_from(value).ok()
}

/// Locator for an EPUB chapter.
///
/// # Errors
///
/// Returns an error when `index` does not fit in `u32`.
pub fn chapter_locator(index: usize) -> Result<String, SessionError> {
    let index = u32::try_from(index).map_err(|_| SessionError::Number)?;
    Ok(format!("chapter:{index}"))
}

/// Locator for a comic or image page.
///
/// # Errors
///
/// Returns an error when `index` does not fit in `u32`.
pub fn page_locator(index: usize) -> Result<String, SessionError> {
    let index = u32::try_from(index).map_err(|_| SessionError::Number)?;
    Ok(format!("page:{index}"))
}

/// Catalog, progress, settings, sessions, servers, and statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySnapshot {
    /// Identity rows.
    pub items: Vec<CatalogItem>,
    /// Books that have a reading row.
    pub progress: Vec<ShelfProgress>,
    /// Setting pairs.
    pub settings: Vec<(String, String)>,
    /// Reading sessions.
    pub sessions: Vec<ReadingSession>,
    /// Saved servers. `secret_ref` is a key, never a password.
    pub servers: Vec<StoredServer>,
    /// History ids, newest first.
    pub history: Vec<String>,
    /// Counts for the statistics screen.
    pub stats: ReadingStats,
}

/// Read the rows the shell lists.
///
/// # Errors
///
/// Returns an error when a catalog read fails.
pub fn load_snapshot(db: &Database) -> Result<LibrarySnapshot, SessionError> {
    let items = db.catalog()?;
    let mut progress = Vec::new();
    for item in &items {
        if let Some(reading) = db.reading(&item.id)? {
            progress.push(ShelfProgress {
                // clone: the shelf owns the id separately from the catalog row
                book_id: item.id.clone(),
                updated_at: reading.updated_at,
                complete: reading.complete,
                page: reading.page,
            });
        }
    }
    let sessions = db.sessions()?;
    let file_types = items
        .iter()
        .map(|item| item.file_type.clone())
        .collect::<Vec<_>>();
    let now = now_ms().unwrap_or(0);
    let stats = summarise(&sessions, &file_types, now);
    Ok(LibrarySnapshot {
        items,
        progress,
        settings: db.settings()?,
        sessions,
        servers: folio_storage::servers(db)?,
        history: folio_library::history_ids(db)?,
        stats,
    })
}

/// Write the current chapter or page immediately.
///
/// # Errors
///
/// Returns an error when the id or locator is empty, or the writer cannot commit.
pub fn save_place(
    db: &Database,
    book_id: &str,
    locator: &str,
    page: u32,
    chapter: &str,
    complete: bool,
) -> Result<(), SessionError> {
    db.set_reading(book_id, locator, page, chapter, complete, now_ms()?)?;
    Ok(())
}

/// Store the opened book's chapter or page count on its catalog row, so a Continue card can
/// show progress. The app's scan never opens books, so this is where the count is learned.
/// Returns whether the row changed.
///
/// # Errors
///
/// Returns an error when the count does not fit in `i64` or the writer cannot commit.
pub fn record_page_count(
    db: &Database,
    item: &CatalogItem,
    count: usize,
) -> Result<bool, SessionError> {
    let count = i64::try_from(count).map_err(|_| SessionError::Number)?;
    if count == 0 || item.page_count == Some(count) {
        return Ok(false);
    }
    let mut updated = item.clone();
    updated.page_count = Some(count);
    db.upsert_item(updated)?;
    Ok(true)
}

/// Write one setting.
///
/// # Errors
///
/// Returns an error when the key or value is empty, or the writer cannot commit.
pub fn save_setting(db: &Database, key: &str, value: &str) -> Result<(), SessionError> {
    db.set_setting(key, value)?;
    Ok(())
}

/// Store a bookmark. The id is unique to this call.
///
/// # Errors
///
/// Returns an error when the book id, locator, or name is empty.
pub fn save_bookmark(
    db: &Database,
    book_id: &str,
    locator: &str,
    name: &str,
) -> Result<(), SessionError> {
    let now = now_ms()?;
    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    db.insert_bookmark(Bookmark {
        id: format!("b-{now}-{seq}"),
        book_id: book_id.to_owned(),
        locator: locator.to_owned(),
        name: name.to_owned(),
    })?;
    Ok(())
}

/// Record history and start a reading session.
///
/// # Errors
///
/// Returns an error when the book id is empty or the writer cannot commit.
pub fn begin_session(db: &Database, book_id: &str) -> Result<String, SessionError> {
    let now = now_ms()?;
    folio_storage::open_history(db, book_id, now)?;
    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    let id = format!("s-{now}-{seq}");
    db.start_session(&id, book_id, now)?;
    Ok(id)
}

/// Close a reading session. An empty id is a session that never started.
///
/// # Errors
///
/// Returns an error when the session is missing or the writer cannot commit.
pub fn end_session(db: &Database, session_id: &str, pages_read: i64) -> Result<(), SessionError> {
    if session_id.is_empty() {
        return Ok(());
    }
    let pages_read = if pages_read < 0 { 0 } else { pages_read };
    db.finish_session(session_id, now_ms()?, pages_read)?;
    Ok(())
}

/// What Sync now tells the reader. This build does not open a socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Sentence shown on the settings screen.
    pub detail: String,
}

/// Replay queued progress. Komga is reported offline. Nothing is sent.
///
/// # Errors
///
/// Returns an error when the catalog cannot be read or a row cannot be queued.
pub fn sync_now(db: &Database) -> Result<SyncReport, SessionError> {
    let mut queue = SyncQueue::new();
    for item in db.catalog()? {
        if let Some(progress) = db.reading(&item.id)? {
            queue.enqueue(SyncQueueItem::progress(
                item.id.as_str(),
                progress.locator.as_str(),
                progress.updated_at,
            ))?;
        }
    }
    let mut adapters = [
        Adapter::new("komga", false, progress_write_supported(ServerKind::Komga)),
        Adapter::new(
            "librarian",
            false,
            progress_write_supported(ServerKind::Librarian),
        ),
        Adapter::new("opds", false, progress_write_supported(ServerKind::OpdsV2)),
        Adapter::new(
            "kavita",
            false,
            progress_write_supported(ServerKind::Kavita),
        ),
    ];
    let statuses = queue.replay(&mut adapters);
    let sent = statuses
        .iter()
        .any(|status| matches!(status, ReplayStatus::Sent { .. }));
    let detail = if sent {
        "Sync did not open a socket. Progress was not delivered.".to_owned()
    } else {
        "Komga is offline. Progress stays on this device.".to_owned()
    };
    Ok(SyncReport { detail })
}

/// Store a server. `secret_ref` is a secret-store key. Empty means anonymous.
///
/// # Errors
///
/// Returns an error when the name, URL, or kind is unusable, or the writer cannot commit.
pub fn add_server(
    db: &Database,
    kind: &str,
    url: &str,
    display_name: &str,
    secret_ref: &str,
) -> Result<(), SessionError> {
    if display_name.is_empty()
        || url.is_empty()
        || !known_kind(kind)
        || url.contains("password=")
        || secret_ref.contains(['\n', '\r'])
    {
        return Err(SessionError::Server);
    }
    let stored_secret = if secret_ref.is_empty() {
        None
    } else {
        Some(secret_ref.to_owned())
    };
    folio_storage::upsert_server(
        db,
        StoredServer {
            id: display_name.to_owned(),
            kind: kind.to_owned(),
            url: url.to_owned(),
            display_name: display_name.to_owned(),
            secret_ref: stored_secret,
        },
    )?;
    Ok(())
}

/// Save edits to an OPDS, Komga or Librarian server. Such a row's id is its name, so a rename
/// moves the row to the new id in one storage job. An empty `secret_ref` keeps the saved one.
/// The kind does not change.
///
/// # Errors
///
/// Returns [`SessionError::NameTaken`] when another row has the new name, and
/// [`SessionError::Server`] when the row is missing, is Kavita, or the edits are unusable.
pub fn edit_server(
    db: &Database,
    id: &str,
    display_name: &str,
    url: &str,
    secret_ref: &str,
) -> Result<(), SessionError> {
    let existing = folio_storage::servers(db)?
        .into_iter()
        .find(|server| server.id == id)
        .ok_or(SessionError::Server)?;
    if existing.kind == "kavita"
        || display_name.is_empty()
        || url.is_empty()
        || url.contains("password=")
        || secret_ref.contains(['\n', '\r'])
    {
        return Err(SessionError::Server);
    }
    let secret_ref = if secret_ref.is_empty() {
        existing.secret_ref
    } else {
        Some(secret_ref.to_owned())
    };
    let moved = folio_storage::swap_server(
        db,
        id,
        StoredServer {
            id: display_name.to_owned(),
            kind: existing.kind,
            url: url.to_owned(),
            display_name: display_name.to_owned(),
            secret_ref,
        },
    );
    match moved {
        Ok(true) => Ok(()),
        Ok(false) => Err(SessionError::Server),
        Err(folio_storage::StorageError::Taken) => Err(SessionError::NameTaken),
        Err(error) => Err(error.into()),
    }
}

/// The error from [`ServerConnection::browse`]. Browse does not open a socket.
pub fn browse_message(
    kind: &str,
    url: &str,
    display_name: &str,
    secret_ref: Option<&str>,
) -> String {
    let auth = match (kind, secret_ref.filter(|value| !value.is_empty())) {
        ("komga", Some(secret)) => AuthConfig::KomgaSession {
            secret_ref: secret.to_owned(),
        },
        ("kavita", Some(secret)) => AuthConfig::KavitaSession {
            secret_ref: secret.to_owned(),
        },
        ("opds", Some(secret)) => AuthConfig::OpdsBasic {
            secret_ref: secret.to_owned(),
        },
        ("librarian", Some(secret)) => AuthConfig::LibrarianSession {
            secret_ref: secret.to_owned(),
        },
        _ => AuthConfig::Anonymous,
    };
    let server_kind = match kind {
        "komga" => ServerKind::Komga,
        "kavita" => ServerKind::Kavita,
        "opds" => ServerKind::OpdsV2,
        _ => ServerKind::Librarian,
    };
    let connection = ServerConnection::new(server_kind, url, display_name, auth);
    match connection.browse() {
        Ok(()) => "Browse is not available.".to_owned(),
        Err(error) => error.to_string(),
    }
}

struct OpenBlock {
    kind: BlockKind,
    tag: String,
}

#[derive(Clone, Copy)]
enum BlockKind {
    Paragraph,
    Heading(u8),
}

struct Tag {
    closing: bool,
    comment: bool,
    name: String,
    next: usize,
}

fn push_quantum(out: &mut String, table: &[u8; 64], value: u32, count: u32) {
    let shifts = [18u32, 12, 6, 0];
    for shift in shifts.iter().take(usize::try_from(count).unwrap_or(0)) {
        let index = (value >> shift) & 0x3F;
        out.push(base64_char(table, index));
    }
}

fn base64_char(table: &[u8; 64], index: u32) -> char {
    let offset = usize::try_from(index).unwrap_or(0);
    char::from(table.get(offset).copied().unwrap_or(b'A'))
}

fn now_ms() -> Result<i64, SessionError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SessionError::Clock)?;
    i64::try_from(elapsed.as_millis()).map_err(|_| SessionError::Clock)
}

fn known_kind(kind: &str) -> bool {
    matches!(kind, "librarian" | "opds" | "komga" | "kavita")
}

fn display_title(title: &str, relative: &str) -> String {
    if title.is_empty() {
        folio_library::filename_label(relative).to_owned()
    } else {
        title.to_owned()
    }
}

fn open_epub_file(path: &Path, relative: &str) -> Result<OpenedBook, SessionError> {
    let mut source = FileSource::open(path)?;
    let publication = open_epub(&mut source)?;
    let mut chapters = Vec::with_capacity(publication.chapters.len());
    let mut pictures = Vec::with_capacity(publication.chapters.len());
    for chapter in publication.chapters {
        let blocks = chapter_blocks(&chapter.html);
        let picture = if blocks.is_empty() {
            chapter.image
        } else {
            None
        };
        if blocks.is_empty() && picture.is_none() {
            continue;
        }
        pictures.push(picture.map(|page| PageBytes {
            name: page.name,
            bytes: page.bytes,
        }));
        chapters.push(blocks);
    }
    Ok(OpenedBook::Text {
        title: display_title(&publication.title, relative),
        chapters,
        pictures,
    })
}

fn open_pdf_file(path: &Path, relative: &str) -> Result<OpenedBook, SessionError> {
    let bytes = fs::read(path)?;
    let page_count = usize::try_from(pdf_page_count(&bytes)?).map_err(|_| SessionError::Number)?;
    if page_count == 0 {
        return Err(SessionError::Format(folio_formats::FormatError::Invalid {
            kind: "pdf",
        }));
    }
    Ok(OpenedBook::Pdf {
        title: folio_library::filename_label(relative).to_owned(),
        bytes: Arc::new(bytes),
        page_count,
    })
}

fn open_mobi_file(path: &Path, relative: &str) -> Result<OpenedBook, SessionError> {
    let mut source = FileSource::open(path)?;
    let book = open_mobi(&mut source)?;
    let mut chapters = vec![chapter_blocks(&book.text)];
    if chapters.iter().all(Vec::is_empty) && !book.text.trim().is_empty() {
        chapters = vec![vec![TextBlock::Paragraph {
            text: book.text.trim().to_owned(),
        }]];
    }
    Ok(OpenedBook::Text {
        title: display_title(&book.title, relative),
        pictures: vec![None; chapters.len()],
        chapters,
    })
}

fn open_comic_file(path: &Path, relative: &str, kind: &str) -> Result<OpenedBook, SessionError> {
    let mut source = FileSource::open(path)?;
    let comic = match kind {
        "cbz" => open_cbz(&mut source, None)?,
        "cb7" => open_cb7(&mut source, None)?,
        "cbt" => open_cbt(&mut source, None)?,
        "cbr" => open_cbr(&mut source, None)?,
        _ => return Err(SessionError::Unsupported),
    };
    let series = comic.series.as_deref().unwrap_or("");
    Ok(pages_book(display_title(series, relative), comic))
}

fn open_image_dir(path: &Path, relative: &str) -> Result<OpenedBook, SessionError> {
    let owned = read_folder_images(path)?;
    let entries = owned
        .iter()
        .map(|(name, bytes)| ImageEntry { name, bytes })
        .collect::<Vec<_>>();
    let comic = open_image_folder(&entries, None)?;
    Ok(pages_book(
        folio_library::filename_label(relative).to_owned(),
        comic,
    ))
}

fn pages_book(title: String, comic: folio_formats::Comic) -> OpenedBook {
    let pages = comic
        .pages
        .into_iter()
        .map(|page| PageBytes {
            name: page.name,
            bytes: page.bytes,
        })
        .collect();
    OpenedBook::Pages {
        title,
        pages: Arc::new(pages),
    }
}

fn read_folder_images(path: &Path) -> Result<Vec<(String, Vec<u8>)>, SessionError> {
    let root = path.canonicalize()?;
    let mut names = Vec::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }
        let os_name = entry.file_name();
        let Some(name) = os_name.to_str() else {
            continue;
        };
        if name.is_empty()
            || name.contains(['\0', '/', '\\'])
            || name == ".."
            || name == "."
            || !is_folder_image(name)
        {
            if name.contains(['\0', '/', '\\']) || name == ".." {
                return Err(SessionError::Escape);
            }
            continue;
        }
        names.push(name.to_owned());
    }
    names.sort();
    let mut owned = Vec::new();
    for name in &names {
        let child = root.join(name);
        let canonical = child.canonicalize()?;
        if !canonical.starts_with(&root) {
            return Err(SessionError::Escape);
        }
        owned.push((name.clone(), fs::read(&canonical)?));
    }
    Ok(owned)
}

fn is_folder_image(name: &str) -> bool {
    matches!(
        page_media_type(name),
        "image/jpeg" | "image/png" | "image/webp" | "image/avif"
    )
}

fn lexical_join(library: &Path, relative: &str) -> Result<PathBuf, SessionError> {
    if relative.is_empty() || relative.contains('\0') {
        return Err(SessionError::Escape);
    }
    let relative_path = Path::new(relative);
    if relative_path.is_absolute() {
        return Err(SessionError::Escape);
    }
    let mut parts = Vec::new();
    for component in relative_path.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str().ok_or(SessionError::Escape)?;
                if text.is_empty() || text.contains(['\0', '/', '\\']) {
                    return Err(SessionError::Escape);
                }
                parts.push(text);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SessionError::Escape);
            }
        }
    }
    if parts.is_empty() {
        return Err(SessionError::Escape);
    }
    let mut joined = library.to_path_buf();
    for part in parts {
        joined.push(part);
    }
    Ok(joined)
}

fn contained(root: &Path, joined: &Path) -> Result<PathBuf, SessionError> {
    if joined.exists() {
        let canonical = joined.canonicalize()?;
        if canonical.starts_with(root) {
            Ok(canonical)
        } else {
            Err(SessionError::Escape)
        }
    } else if let Some(parent) = joined.parent() {
        if parent.exists() {
            let canonical_parent = parent.canonicalize()?;
            if !canonical_parent.starts_with(root) {
                return Err(SessionError::Escape);
            }
            let name = joined.file_name().ok_or(SessionError::Escape)?;
            Ok(canonical_parent.join(name))
        } else if joined.starts_with(root) {
            Ok(joined.to_path_buf())
        } else {
            Err(SessionError::Escape)
        }
    } else {
        Err(SessionError::Escape)
    }
}

fn error_chain(error: &impl std::error::Error) -> String {
    let mut detail = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        detail.push_str(": ");
        detail.push_str(&next.to_string());
        source = next.source();
    }
    detail
}

fn flush_loose(blocks: &mut Vec<TextBlock>, buffer: &mut String, current: Option<OpenBlock>) {
    if let Some(open) = current {
        push_block(blocks, open.kind, buffer);
    } else if finished_text(buffer).is_some() {
        push_block(blocks, BlockKind::Paragraph, buffer);
    }
    buffer.clear();
}

fn push_block(blocks: &mut Vec<TextBlock>, kind: BlockKind, raw: &str) {
    let Some(text) = finished_text(raw) else {
        return;
    };
    blocks.push(match kind {
        BlockKind::Paragraph => TextBlock::Paragraph { text },
        BlockKind::Heading(level) => TextBlock::Heading { level, text },
    });
}

fn finished_text(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut pending_space = false;
    for character in raw.chars() {
        if character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(character);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn block_kind(name: &str) -> Option<BlockKind> {
    if let Some(level) = heading_level(name) {
        return Some(BlockKind::Heading(level));
    }
    match name {
        "p" | "li" | "blockquote" | "pre" | "figcaption" | "td" | "dd" => {
            Some(BlockKind::Paragraph)
        }
        _ => None,
    }
}

fn heading_level(name: &str) -> Option<u8> {
    let bytes = name.as_bytes();
    if bytes.len() != 2 || bytes[0] != b'h' {
        return None;
    }
    match bytes[1] {
        b'1' => Some(1),
        b'2' => Some(2),
        b'3' => Some(3),
        b'4' => Some(4),
        b'5' => Some(5),
        b'6' => Some(6),
        _ => None,
    }
}

fn parse_tag(bytes: &[u8], start: usize) -> Option<Tag> {
    if bytes.get(start) != Some(&b'<') {
        return None;
    }
    let mut index = start.saturating_add(1);
    if bytes.get(index..index.saturating_add(3)) == Some(b"!--") {
        let next = find_slice(bytes, index, b"-->")
            .map(|position| position.saturating_add(3))
            .unwrap_or(bytes.len());
        return Some(Tag {
            closing: false,
            comment: true,
            name: String::new(),
            next,
        });
    }
    if bytes.get(index) == Some(&b'!') {
        let next = find_byte(bytes, index, b'>')
            .map(|position| position.saturating_add(1))
            .unwrap_or(bytes.len());
        return Some(Tag {
            closing: false,
            comment: true,
            name: String::new(),
            next,
        });
    }
    let closing = if bytes.get(index) == Some(&b'/') {
        index = index.saturating_add(1);
        true
    } else {
        false
    };
    let name_start = index;
    while index < bytes.len() && bytes[index].is_ascii_alphanumeric() {
        index = index.saturating_add(1);
    }
    if index == name_start {
        return None;
    }
    let mut name = String::new();
    for byte in &bytes[name_start..index] {
        name.push(char::from(*byte).to_ascii_lowercase());
    }
    let next = find_byte(bytes, index, b'>')
        .map(|position| position.saturating_add(1))
        .unwrap_or(bytes.len());
    Some(Tag {
        closing,
        comment: false,
        name,
        next,
    })
}

fn skip_until_close(bytes: &[u8], from: usize, name: &str) -> usize {
    let marker = format!("</{name}");
    let marker = marker.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        let rest = &bytes[index..];
        if rest.len() >= marker.len() && rest[..marker.len()].eq_ignore_ascii_case(marker) {
            return find_byte(bytes, index, b'>')
                .map(|position| position.saturating_add(1))
                .unwrap_or(bytes.len());
        }
        index = index.saturating_add(1);
    }
    bytes.len()
}

fn decode_entity(bytes: &[u8], start: usize) -> Option<(char, usize)> {
    if bytes.get(start) != Some(&b'&') {
        return None;
    }
    let end = find_byte(bytes, start, b';')?;
    let body_len = end.saturating_sub(start);
    if body_len > 16 {
        return None;
    }
    let body = &bytes[start.saturating_add(1)..end];
    let next = end.saturating_add(1);
    if let Some(hex) = body
        .strip_prefix(b"#x")
        .or_else(|| body.strip_prefix(b"#X"))
    {
        let text = std::str::from_utf8(hex).ok()?;
        let code = u32::from_str_radix(text, 16).ok()?;
        return Some((char::from_u32(code)?, next));
    }
    if let Some(decimal) = body.strip_prefix(b"#") {
        let text = std::str::from_utf8(decimal).ok()?;
        let code = text.parse::<u32>().ok()?;
        return Some((char::from_u32(code)?, next));
    }
    let name = std::str::from_utf8(body).ok()?;
    let character = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{00a0}',
        _ => return None,
    };
    Some((character, next))
}

fn push_utf8(buffer: &mut String, bytes: &[u8], index: &mut usize) {
    let Some(first) = bytes.get(*index).copied() else {
        return;
    };
    let width = utf8_width(first);
    let end = index.saturating_add(width);
    if end <= bytes.len() {
        if let Ok(text) = std::str::from_utf8(&bytes[*index..end]) {
            buffer.push_str(text);
            *index = end;
            return;
        }
    }
    *index = index.saturating_add(1);
}

fn utf8_width(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte & 0xE0 == 0xC0 {
        2
    } else if byte & 0xF0 == 0xE0 {
        3
    } else if byte & 0xF8 == 0xF0 {
        4
    } else {
        1
    }
}

fn find_byte(bytes: &[u8], from: usize, needle: u8) -> Option<usize> {
    bytes
        .get(from..)?
        .iter()
        .position(|byte| *byte == needle)
        .map(|offset| from.saturating_add(offset))
}

fn find_slice(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let haystack = bytes.get(from..)?;
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from.saturating_add(offset))
}

#[cfg(test)]
mod tests {
    use super::{
        add_server, book_path, browse_message, chapter_blocks, edit_server, encode_base64,
        load_snapshot, open_at, open_database, page_data_url, record_page_count, save_place,
        scan_libraries, sync_now, FileSource, OpenedBook, SessionError, TextBlock,
    };
    use folio_core::SeekableSource;
    use folio_storage::CatalogItem;
    use std::fs;
    use std::io::Write;
    use std::path::Path;

    #[test]
    fn chapter_blocks_decode_entities_and_keep_multibyte_text() {
        assert_eq!(
            chapter_blocks("<p>Tom &amp; Jerry &#233;</p>"),
            vec![TextBlock::Paragraph {
                text: "Tom & Jerry é".to_owned()
            }]
        );
        assert_eq!(
            chapter_blocks("<h1>日本語</h1>"),
            vec![TextBlock::Heading {
                level: 1,
                text: "日本語".to_owned()
            }]
        );
        for code in [0xE9u32, 0x41, 0x20AC, 0x3042] {
            let character = char::from_u32(code).expect("scalar");
            let blocks = chapter_blocks(&format!("<p>&#{code};</p>"));
            assert_eq!(
                blocks,
                vec![TextBlock::Paragraph {
                    text: character.to_string()
                }]
            );
        }
    }

    #[test]
    fn chapter_blocks_skip_script_and_style_and_empty_input() {
        assert!(chapter_blocks("").is_empty());
        assert!(chapter_blocks("<p> </p>").is_empty());
        assert!(chapter_blocks("<script>only</script>").is_empty());
        assert_eq!(
            chapter_blocks("<p>Keep</p><script>drop this</script><style>body{}</style><p>Also</p>"),
            vec![
                TextBlock::Paragraph {
                    text: "Keep".to_owned()
                },
                TextBlock::Paragraph {
                    text: "Also".to_owned()
                }
            ]
        );
        assert_eq!(
            chapter_blocks("<p>&lt;b&gt;hi&lt;/b&gt;</p>"),
            vec![TextBlock::Paragraph {
                text: "<b>hi</b>".to_owned()
            }]
        );
        assert_eq!(
            chapter_blocks(
                "<?xml version='1.0'?><!DOCTYPE html><html><head><title>Secret</title></head><body><h1>Alice</h1><p>Down</p></body></html>"
            ),
            vec![
                TextBlock::Heading {
                    level: 1,
                    text: "Alice".to_owned()
                },
                TextBlock::Paragraph {
                    text: "Down".to_owned()
                }
            ]
        );
    }

    #[test]
    fn file_source_short_read_and_past_end() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("short.bin");
        fs::write(&path, b"abcd").expect("write");
        let mut source = FileSource::open(&path).expect("open");
        let mut buf = [0u8; 8];
        let count = source.read_at(2, &mut buf).expect("short");
        assert_eq!(count, 2);
        assert_eq!(&buf[..2], b"cd");
        assert_eq!(source.read_at(4, &mut buf).expect("end"), 0);
        assert!(source.read_at(5, &mut buf).is_err());
    }

    #[test]
    fn identity_scan_keeps_books_and_download_together() {
        let parent = tempfile::tempdir().expect("parent");
        fs::create_dir(parent.path().join("Books")).expect("books");
        fs::write(parent.path().join("Books/Hello.epub"), b"not-a-real-epub").expect("epub");
        fs::create_dir(parent.path().join("Download")).expect("download");
        fs::write(parent.path().join("Download/Note.pdf"), b"%PDF-1.4").expect("pdf");
        let files = tempfile::tempdir().expect("files");
        let db = open_database(files.path()).expect("db");
        let scan =
            scan_libraries(parent.path(), &db, &super::cover_dir(files.path())).expect("scan");
        let ids = scan
            .outcome
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"Books/Hello.epub"));
        assert!(ids.contains(&"Download/Note.pdf"));
        assert!(scan.outcome.opened.is_empty());
        assert!(scan.unreadable.is_empty());
        let epub = scan
            .outcome
            .rows
            .iter()
            .find(|row| row.id == "Books/Hello.epub")
            .expect("epub row");
        assert_eq!(epub.file_type, "epub");
        assert_eq!(epub.size, Some(15));
        let again =
            scan_libraries(parent.path(), &db, &super::cover_dir(files.path())).expect("again");
        let ids = again
            .outcome
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"Books/Hello.epub"));
        assert!(ids.contains(&"Download/Note.pdf"));
        let database_path = files.path().join("folio.sqlite");
        assert!(database_path.is_file());
        assert!(!database_path.starts_with(parent.path()));
    }

    #[test]
    fn book_path_rejects_parent_segments_and_outside_symlinks() {
        assert!(book_path(Path::new("/library"), "..").is_err());
        assert!(book_path(Path::new("/library"), "Books/../../etc/passwd").is_err());
        assert!(book_path(Path::new("/library"), "/etc/passwd").is_err());
        assert!(book_path(Path::new("/library"), "Books/Hello.epub").is_ok());

        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("secret.epub");
        fs::write(&secret, b"no").expect("secret");
        fs::create_dir(root.path().join("Books")).expect("books");
        std::os::unix::fs::symlink(&secret, root.path().join("Books/link.epub")).expect("link");
        fs::write(root.path().join("Books/Hello.epub"), b"epub").expect("hello");
        assert!(book_path(root.path(), "Books/link.epub").is_err());
        assert!(book_path(root.path(), "Books/../secret.epub").is_err());
        let hello = book_path(root.path(), "Books/Hello.epub").expect("hello path");
        assert!(hello.ends_with("Hello.epub"));
        assert!(hello.starts_with(root.path().canonicalize().expect("root path")));
    }

    #[test]
    fn pdf_mobi_and_cbr_open_as_books() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join("Books")).expect("books");
        fs::write(root.path().join("Books/Note.pdf"), one_page_pdf()).expect("pdf");
        fs::write(root.path().join("Books/Cafe.mobi"), sample_mobi()).expect("mobi");
        fs::write(root.path().join("Books/Old.cbr"), sample_cbr()).expect("cbr");
        fs::write(root.path().join("Books/Broken.cbr"), b"Rar!").expect("junk");

        let pdf = open_at(root.path(), "Books/Note.pdf", "pdf").expect("open pdf");
        assert!(matches!(pdf, OpenedBook::Pdf { page_count: 1, .. }));
        let png = pdf.pdf_png(0).expect("png");
        assert!(png.starts_with(b"\x89PNG"));

        let mobi = open_at(root.path(), "Books/Cafe.mobi", "mobi").expect("open mobi");
        assert_eq!(mobi.title(), "Cafe");
        assert_eq!(
            mobi.blocks(0),
            &[TextBlock::Paragraph {
                text: "Hello café".to_owned()
            }]
        );

        let cbr = open_at(root.path(), "Books/Old.cbr", "cbr").expect("open cbr");
        match &cbr {
            OpenedBook::Pages { pages, .. } => {
                assert_eq!(pages.len(), 1);
                assert_eq!(pages[0].bytes, b"page");
            }
            other => panic!("expected pages, got {other:?}"),
        }
        assert!(open_at(root.path(), "Books/Broken.cbr", "cbr").is_err());
        assert!(open_at(root.path(), "Books/../../etc/passwd", "epub").is_err());
    }

    #[test]
    fn sync_now_reports_komga_offline_and_browse_has_no_password() {
        let files = tempfile::tempdir().expect("files");
        let db = open_database(files.path()).expect("db");
        db.upsert_item(sample("Books/Hello.epub", "epub"))
            .expect("item");
        save_place(&db, "Books/Hello.epub", "chapter:0", 0, "chapter:0", false).expect("place");
        let report = sync_now(&db).expect("sync");
        assert!(report.detail.contains("Komga is offline"));
        assert!(!report.detail.to_ascii_lowercase().contains("sent"));
        add_server(
            &db,
            "librarian",
            "http://127.0.0.1:9",
            "Local",
            "keyring:librarian:local",
        )
        .expect("server");
        let message = browse_message(
            "librarian",
            "http://127.0.0.1:9",
            "Local",
            Some("keyring:librarian:local"),
        );
        assert!(message.contains("browse"));
        assert!(!message.to_ascii_lowercase().contains("password"));
        assert!(add_server(
            &db,
            "librarian",
            "http://127.0.0.1:9/?password=secret",
            "Bad",
            ""
        )
        .is_err());
    }

    #[test]
    fn editing_a_server_moves_it_to_its_new_name_and_keeps_the_secret() {
        let files = tempfile::tempdir().expect("files");
        let db = open_database(files.path()).expect("db");
        add_server(
            &db,
            "opds",
            "http://127.0.0.1:9/opds",
            "Local",
            "keyring:opds:local",
        )
        .expect("local");
        add_server(&db, "komga", "http://127.0.0.1:9/komga", "Comics", "").expect("comics");
        edit_server(&db, "Local", "Attic", "http://127.0.0.1:8/opds", "").expect("edit");
        let rows = folio_storage::servers(&db).expect("rows");
        let attic = rows
            .iter()
            .find(|row| row.id == "Attic")
            .expect("moved row");
        assert_eq!(attic.display_name, "Attic");
        assert_eq!(attic.kind, "opds");
        assert_eq!(attic.url, "http://127.0.0.1:8/opds");
        assert_eq!(attic.secret_ref.as_deref(), Some("keyring:opds:local"));
        assert!(rows.iter().all(|row| row.id != "Local"));
        assert!(matches!(
            edit_server(&db, "Attic", "Comics", "http://127.0.0.1:8/opds", ""),
            Err(SessionError::NameTaken)
        ));
        edit_server(
            &db,
            "Attic",
            "Attic",
            "http://127.0.0.1:8/opds",
            "keyring:opds:attic",
        )
        .expect("secret");
        let rows = folio_storage::servers(&db).expect("rows");
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .any(|row| row.secret_ref.as_deref() == Some("keyring:opds:attic")));
        assert!(matches!(
            edit_server(&db, "Missing", "Missing", "http://127.0.0.1:8/opds", ""),
            Err(SessionError::Server)
        ));
        assert!(matches!(
            edit_server(&db, "Attic", "Attic", "http://127.0.0.1:8/?password=x", ""),
            Err(SessionError::Server)
        ));
    }

    #[test]
    fn base64_and_data_url_use_the_standard_alphabet() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"hello"), "aGVsbG8=");
        assert_eq!(page_data_url("a.png", b"f"), "data:image/png;base64,Zg==");
    }

    fn one_page_pdf() -> Vec<u8> {
        let stream = b"BT /F1 24 Tf 40 100 Td (Hi) Tj ET";
        let length = stream.len();
        let stream_text = std::str::from_utf8(stream).expect("ascii stream");
        let objects = [
            "1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n".to_string(),
            "2 0 obj<</Type/Pages/Count 1/Kids[3 0 R]>>endobj\n".to_string(),
            "3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj\n".to_string(),
            format!("4 0 obj<</Length {length}>>stream\n{stream_text}\nendstream\nendobj\n"),
            "5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\n".to_string(),
        ];
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = Vec::new();
        for object in &objects {
            offsets.push(out.len());
            out.extend_from_slice(object.as_bytes());
        }
        let xref_at = out.len();
        let mut xref = format!("xref\n0 {}\n", objects.len() + 1);
        xref.push_str("0000000000 65535 f \n");
        for offset in offsets {
            xref.push_str(&format!("{offset:010} 00000 n \n"));
        }
        let trailer = format!(
            "trailer<</Size {}/Root 1 0 R>>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        );
        out.extend_from_slice(xref.as_bytes());
        out.extend_from_slice(trailer.as_bytes());
        out
    }

    fn sample_mobi() -> Vec<u8> {
        let title = b"Cafe";
        let text = "<p>Hello café</p>".as_bytes();
        let mut record0 = vec![0u8; 16 + 232];
        record0[1] = 1;
        let text_len = u32::try_from(text.len()).expect("text len");
        record0[4..8].copy_from_slice(&text_len.to_be_bytes());
        record0[9] = 1;
        record0[10] = 0x10;
        record0[16..20].copy_from_slice(b"MOBI");
        record0[20..24].copy_from_slice(&232u32.to_be_bytes());
        record0[24..28].copy_from_slice(&2u32.to_be_bytes());
        record0[28..32].copy_from_slice(&65001u32.to_be_bytes());
        let name_at = u32::try_from(record0.len()).expect("name");
        record0.extend_from_slice(title);
        record0[100..104].copy_from_slice(&name_at.to_be_bytes());
        let title_len = u32::try_from(title.len()).expect("title len");
        record0[104..108].copy_from_slice(&title_len.to_be_bytes());
        let record0_at = 78 + 16;
        let text_at = record0_at + record0.len();
        let mut bytes = vec![0u8; 78];
        bytes[60..64].copy_from_slice(b"BOOK");
        bytes[64..68].copy_from_slice(b"MOBI");
        bytes[77] = 2;
        bytes.extend_from_slice(&u32::try_from(record0_at).expect("record").to_be_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&u32::try_from(text_at).expect("text").to_be_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&record0);
        bytes.extend_from_slice(text);
        bytes
    }

    fn sample_cbr() -> Vec<u8> {
        let mut writer = rar_codec::ArchiveBuilder::new();
        writer.add_stored("1.png", b"page").expect("store");
        writer.finish()
    }

    fn sample(id: &str, file_type: &str) -> CatalogItem {
        CatalogItem {
            id: id.to_owned(),
            source: id.to_owned(),
            label: id.to_owned(),
            file_type: file_type.to_owned(),
            size: None,
            modified_ms: None,
            series_guess: String::new(),
            series: None,
            title: id.to_owned(),
            page_count: None,
            image_child_count: None,
            fingerprint: "test".to_owned(),
            enriched_fingerprint: None,
        }
    }

    #[test]
    fn file_source_reports_a_short_write_buffer() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("wide.bin");
        let mut file = fs::File::create(&path).expect("create");
        file.write_all(&[7u8; 3]).expect("write");
        drop(file);
        let mut source = FileSource::open(&path).expect("open");
        let mut buf = [0u8; 1];
        assert_eq!(source.read_at(1, &mut buf).expect("one"), 1);
        assert_eq!(buf[0], 7);
    }

    #[test]
    fn snapshot_carries_the_saved_page_for_continue_cards() {
        let files = tempfile::tempdir().expect("files");
        let db = open_database(files.path()).expect("db");
        db.upsert_item(sample("Books/Hello.epub", "epub"))
            .expect("item");
        save_place(&db, "Books/Hello.epub", "chapter:2", 2, "chapter:2", false).expect("place");
        let snapshot = load_snapshot(&db).expect("snapshot");
        assert_eq!(snapshot.progress[0].page, Some(2));
    }

    #[test]
    fn record_page_count_fills_and_corrects_the_count() {
        let files = tempfile::tempdir().expect("files");
        let db = open_database(files.path()).expect("db");
        let item = sample("Books/Hello.epub", "epub");
        db.upsert_item(item.clone()).expect("item");

        assert!(record_page_count(&db, &item, 24).expect("record"));
        let stored = db.catalog().expect("catalog").remove(0);
        assert_eq!(stored.page_count, Some(24));
        assert!(!record_page_count(&db, &stored, 24).expect("same"));
        assert!(!record_page_count(&db, &stored, 0).expect("zero"));

        save_place(&db, "Books/Hello.epub", "chapter:2", 2, "chapter:2", false).expect("place");
        let snapshot = load_snapshot(&db).expect("snapshot");
        let line = crate::shell::progress_line(
            &snapshot.items[0].file_type,
            snapshot.progress[0].page,
            snapshot.items[0].page_count,
        )
        .expect("line");
        assert_eq!(line.label, "Chapter 3 of 24");
    }
}
