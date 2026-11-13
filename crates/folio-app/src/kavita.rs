//! Kavita account, mirror, downloads, and progress.
//!
//! The Auth Key is a file under the secret store. The database stores `secret:kavita:<id>`.
//! Opening a page does not write a catalog row.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::io::Error as IoError;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use folio_library::{leave_foreground, recover, run_kavita, same_origin};
use folio_platform::SecretStore;
use folio_reader::{can_open, download_complete, StoredPage};
use folio_server::{
    all_series_request, authed_get, bookmarks_request, collections_request, cover_request,
    dashboard_request, epub_page_request, execute, filters_request, image_page_request,
    libraries_request, mark_chapter_request, mark_series_request, mark_volume_request,
    on_deck_request, page_len, parse_bookmarks, parse_dashboard, parse_filters,
    parse_grouped_series_ids, parse_libraries, parse_series, parse_series_ids, parse_titled,
    parse_volumes, pdf_request, progress_request, reading_lists_request, recently_added_request,
    recently_updated_request, volumes_request, want_to_read_request, KavitaBookmark, KavitaLibrary,
    KavitaNamed, KavitaProgress, KavitaRequest, KavitaSeries, KavitaStream, ServerError,
    KAVITA_PAGE,
};
use folio_storage::{
    acknowledge_kavita_mark, acknowledge_kavita_progress, apply_kavita_pull, drop_kavita_series,
    drop_orphan_kavita_downloads, enqueue_kavita_download, enqueue_kavita_mark,
    enqueue_kavita_progress, forget_kavita_server, kavita_marks, kavita_mirror_mutated,
    kavita_outbox, publish_kavita_mirror, remove_server, replace_kavita_chapters,
    reset_kavita_server, servers, swap_server, upsert_server, KavitaBookmarkRow, KavitaChapterRow,
    KavitaHomeRow, KavitaKeep, KavitaLibraryRow, KavitaMarkRow, KavitaMirror, KavitaNamedRow,
    KavitaOutboxRow, KavitaSeriesRow, StoredServer,
};

use crate::session::SessionError;

const SECRET_KIND: &str = "kavita";
const ON_DECK: &str = "on-deck";
const RECENTLY_ADDED: &str = "added";
const RECENTLY_UPDATED: &str = "updated";

/// A Kavita reply the mirror cannot be published without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorList {
    /// `Library/libraries`.
    Libraries,
    /// `Series/all-v2`.
    Series,
}

impl fmt::Display for MirrorList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Libraries => "library list",
            Self::Series => "series list",
        })
    }
}

/// One request of a sync, as a failure message names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorStep {
    /// A list the mirror cannot do without.
    List(MirrorList),
    /// An optional section.
    Section(MirrorSection),
    /// The volumes of the series being opened.
    Chapters,
}

impl fmt::Display for MirrorStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::List(list) => write!(f, "the {list}"),
            Self::Section(section) => f.write_str(section.label()),
            Self::Chapters => f.write_str("the chapters"),
        }
    }
}

/// A part of the mirror that is not needed to list and open series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorSection {
    /// Dashboard streams.
    Home,
    /// On Deck shelf.
    OnDeck,
    /// Recently added shelf.
    RecentlyAdded,
    /// Recently updated shelf.
    RecentlyUpdated,
    /// Collections.
    Collections,
    /// Reading lists.
    ReadingLists,
    /// Smart filters.
    Filters,
    /// Want to Read.
    WantToRead,
    /// Bookmarks.
    Bookmarks,
}

impl MirrorSection {
    /// The chip that shows this section on the Kavita screen.
    fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::OnDeck => "On Deck",
            Self::RecentlyAdded => "Added",
            Self::RecentlyUpdated => "Updated",
            Self::Collections => "Collections",
            Self::ReadingLists => "Lists",
            Self::Filters => "Filters",
            Self::WantToRead => "Want to Read",
            Self::Bookmarks => "Bookmarks",
        }
    }
}

/// What a sync could not read from Kavita.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirrorGaps {
    /// Sections whose reply could not be read. Publishing keeps their last published rows.
    pub stale: Vec<MirrorSection>,
}

impl MirrorGaps {
    /// Words for the gaps, or `None` when every reply was read.
    pub fn note(&self) -> Option<String> {
        let (last, rest) = self.stale.split_last()?;
        let labels: Vec<&str> = rest.iter().map(|section| section.label()).collect();
        let list = if labels.is_empty() {
            last.label().to_owned()
        } else {
            format!("{} and {}", labels.join(", "), last.label())
        };
        Some(format!("{list} could not be read."))
    }
}

/// One reconciled mirror. Nothing here is published until [`publish_mirror`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorSnapshot {
    /// Libraries.
    pub libraries: Vec<KavitaLibrary>,
    /// Every series, including ones that also appear on a shelf.
    pub series: Vec<KavitaSeries>,
    /// Dashboard streams.
    pub home: Vec<KavitaStream>,
    /// Collections.
    pub collections: Vec<KavitaNamed>,
    /// Reading lists.
    pub reading_lists: Vec<KavitaNamed>,
    /// Smart filters.
    pub filters: Vec<KavitaNamed>,
    /// Want to Read series ids, in server order.
    pub wants: Vec<i64>,
    /// Bookmarks.
    pub bookmarks: Vec<KavitaBookmark>,
    /// On Deck series ids, in server order.
    pub on_deck: Vec<i64>,
    /// Recently added series ids.
    pub recently_added: Vec<i64>,
    /// Recently updated series ids.
    pub recently_updated: Vec<i64>,
    /// Replies that could not be read.
    pub gaps: MirrorGaps,
}

impl MirrorSnapshot {
    /// An empty mirror.
    pub fn empty() -> Self {
        Self {
            libraries: Vec::new(),
            series: Vec::new(),
            home: Vec::new(),
            collections: Vec::new(),
            reading_lists: Vec::new(),
            filters: Vec::new(),
            wants: Vec::new(),
            bookmarks: Vec::new(),
            on_deck: Vec::new(),
            recently_added: Vec::new(),
            recently_updated: Vec::new(),
            gaps: MirrorGaps::default(),
        }
    }
}

/// Save a Kavita server. The returned id is not the display name, and the backup has no raw key.
///
/// # Errors
///
/// Returns an error when the name, URL, or key is unusable, or the secret file cannot be written.
pub fn add_kavita(
    db: &folio_storage::Database,
    store: &SecretStore,
    name: &str,
    url: &str,
    api_key: &str,
) -> Result<String, SessionError> {
    validate_account(name, url, api_key)?;
    let id = new_id()?;
    store.write(SECRET_KIND, &id, api_key)?;
    upsert_server(
        db,
        StoredServer {
            // to_owned: the server row keeps the id after this function returns
            id: id.clone(),
            kind: SECRET_KIND.to_owned(),
            url: url.to_owned(),
            display_name: name.to_owned(),
            secret_ref: Some(secret_reference(&id)),
        },
    )?;
    Ok(id)
}

/// Change the display name. The id, mirror, downloads, and key stay.
///
/// # Errors
///
/// Returns an error when the server is missing or the name is empty.
pub fn rename_kavita(
    db: &folio_storage::Database,
    id: &str,
    name: &str,
) -> Result<(), SessionError> {
    if name.is_empty() {
        return Err(SessionError::Server);
    }
    let existing = kavita_server(db, id)?;
    upsert_server(
        db,
        StoredServer {
            id: existing.id,
            kind: existing.kind,
            url: existing.url,
            display_name: name.to_owned(),
            secret_ref: existing.secret_ref,
        },
    )?;
    Ok(())
}

/// Replace the URL or Auth Key. Either change drops that server's mirror, downloads, and outbox.
///
/// The new key is written before the one storage job that drops the mirror and stores the URL,
/// so a sync that fetched with the old account sees the change whenever its publish lands.
///
/// # Errors
///
/// Returns an error when the server is missing or the new account is unusable.
pub fn replace_kavita(
    db: &folio_storage::Database,
    store: &SecretStore,
    id: &str,
    url: &str,
    api_key: &str,
) -> Result<(), SessionError> {
    let existing = kavita_server(db, id)?;
    validate_account(&existing.display_name, url, api_key)?;
    let previous = match store.read(SECRET_KIND, id) {
        Ok(value) => value,
        Err(folio_platform::SecretError::Missing) => String::new(),
        Err(error) => return Err(error.into()),
    };
    let changed = existing.url != url || previous != api_key;
    store.write(SECRET_KIND, id, api_key)?;
    let row = StoredServer {
        id: existing.id,
        kind: existing.kind,
        url: url.to_owned(),
        display_name: existing.display_name,
        secret_ref: Some(secret_reference(id)),
    };
    let stored = if changed {
        reset_kavita_server(db, row)
    } else {
        upsert_server(db, row)
    };
    if let Err(error) = stored {
        if !previous.is_empty() && store.write(SECRET_KIND, id, &previous).is_err() {
            tracing::warn!("the previous Auth Key could not be restored");
        }
        return Err(error.into());
    }
    Ok(())
}

/// Save edits to a Kavita server: a new name renames it, and a new URL or a typed key replaces
/// the account. An empty `api_key` keeps the saved key.
///
/// # Errors
///
/// Returns an error when the server is missing, the name is empty, or the account is unusable.
pub fn update_kavita(
    db: &folio_storage::Database,
    store: &SecretStore,
    id: &str,
    name: &str,
    url: &str,
    api_key: &str,
) -> Result<(), SessionError> {
    let existing = kavita_server(db, id)?;
    if name != existing.display_name {
        rename_kavita(db, id, name)?;
    }
    if url != existing.url || !api_key.is_empty() {
        let key = if api_key.is_empty() {
            read_key(store, id)?
        } else {
            api_key.to_owned()
        };
        replace_kavita(db, store, id, url, &key)?;
    }
    Ok(())
}

/// Ask Kavita how many libraries `url` has. Nothing is saved.
///
/// Invalid certificates are rejected. The Auth Key is not copied into the error text.
///
/// # Errors
///
/// Returns an error when the URL or key cannot be used, or the server does not answer
/// with a library list.
pub fn probe_kavita(url: &str, api_key: &str) -> Result<usize, SessionError> {
    if api_key.is_empty() {
        return Err(ServerError::Secret.into());
    }
    if url.contains("password=") {
        return Err(SessionError::Server);
    }
    let request = libraries_request(url, api_key)?;
    let bytes = execute(&request, false)?;
    let text = String::from_utf8(bytes).map_err(|_| ServerError::Feed)?;
    Ok(parse_libraries(&text)?.len())
}

/// Words for a Kavita failure. A URL or Auth Key in `error` is not repeated.
pub fn connection_failure(error: &SessionError) -> String {
    let text = error.to_string();
    if text.contains("://") {
        return "Kavita could not be contacted.".to_owned();
    }
    match error {
        SessionError::Remote(ServerError::Refused) => "The server refused the Auth Key.".to_owned(),
        SessionError::Remote(ServerError::Dns) => "The server name could not be found.".to_owned(),
        SessionError::Remote(ServerError::Transport) => {
            "The server could not be reached.".to_owned()
        }
        SessionError::Remote(ServerError::Secret) | SessionError::Secret(_) => {
            "The Auth Key is missing.".to_owned()
        }
        SessionError::Remote(ServerError::Feed) => {
            "Kavita answered, but the reply could not be read.".to_owned()
        }
        SessionError::Remote(_) => "Kavita could not be contacted.".to_owned(),
        SessionError::Changed => {
            "The server was changed while Folio read it. Try again.".to_owned()
        }
        SessionError::Hidden => "That series is not in the library.".to_owned(),
        SessionError::Mirror { step, error } => match error {
            ServerError::Feed => format!("Kavita answered, but {step} could not be read."),
            ServerError::Transport => {
                format!("The server stopped answering while Folio read {step}.")
            }
            other => connection_failure(&SessionError::Remote(other.clone())),
        },
        _ => text,
    }
}

/// Status after a refresh. `fetch_error` wins; otherwise `gaps` follows the status. The word
/// "sent" is not used.
pub fn refresh_status(
    saw_kavita: bool,
    delivered: bool,
    fetch_error: Option<&str>,
    push_failed: bool,
    gaps: Option<&str>,
    local_detail: &str,
) -> String {
    if let Some(error) = fetch_error {
        if error.contains("://") {
            return "Kavita could not be contacted.".to_owned();
        }
        return error.to_owned();
    }
    let status = if delivered && push_failed {
        "Some progress was delivered. The rest stays on this device."
    } else if delivered && saw_kavita {
        "Progress was delivered. Kavita was refreshed."
    } else if delivered {
        "Progress was delivered."
    } else if saw_kavita {
        "Kavita was refreshed."
    } else {
        local_detail
    };
    match gaps {
        Some(note) => format!("{status} {note}"),
        None => status.to_owned(),
    }
}

/// An OPDS row whose secret column holds the Auth Key itself.
pub fn adoptable_opds(kind: &str, secret: Option<&str>) -> bool {
    kind == "opds" && raw_secret(secret)
}

/// Move the only raw OPDS row onto Kavita. Two raw rows, or a Kavita row already saved, stay put.
///
/// # Errors
///
/// Returns an error when the single row cannot be saved as Kavita.
pub fn rehome_raw_opds(
    db: &folio_storage::Database,
    store: &SecretStore,
) -> Result<Option<String>, SessionError> {
    let rows = servers(db)?;
    if rows.iter().any(|server| server.kind == SECRET_KIND) {
        return Ok(None);
    }
    let mut raw = rows
        .into_iter()
        .filter(|server| adoptable_opds(&server.kind, server.secret_ref.as_deref()))
        .collect::<Vec<_>>();
    if raw.len() != 1 {
        return Ok(None);
    }
    let Some(old) = raw.pop() else {
        return Ok(None);
    };
    adopt(db, store, old).map(Some)
}

/// Move one raw OPDS row onto Kavita. The caller picked the row.
///
/// # Errors
///
/// Returns an error when the row is missing, is not a raw OPDS secret, or cannot be saved.
pub fn rehome_server(
    db: &folio_storage::Database,
    store: &SecretStore,
    id: &str,
) -> Result<String, SessionError> {
    let old = servers(db)?
        .into_iter()
        .find(|server| server.id == id)
        .ok_or(SessionError::Server)?;
    if !adoptable_opds(&old.kind, old.secret_ref.as_deref()) {
        return Err(SessionError::Server);
    }
    adopt(db, store, old)
}

/// Remove one saved server. A Kavita row also drops its mirror, downloads, and Auth Key file.
///
/// # Errors
///
/// Returns [`SessionError::Server`] when `id` does not match a saved row.
pub fn forget_server(
    db: &folio_storage::Database,
    store: &SecretStore,
    files_dir: &Path,
    id: &str,
) -> Result<(), SessionError> {
    let kavita = servers(db)?
        .into_iter()
        .find(|server| server.id == id)
        .ok_or(SessionError::Server)?
        .kind
        == SECRET_KIND;
    if !kavita {
        remove_server(db, id)?;
        return Ok(());
    }
    // The key goes first, so a sync that is still reading this server finds it changed.
    store.delete(SECRET_KIND, id)?;
    forget_kavita_server(db, id)?;
    remove_server_downloads(files_dir, id)?;
    Ok(())
}

fn remove_server_downloads(files_dir: &Path, server_id: &str) -> Result<(), SessionError> {
    if !safe_segment(server_id) {
        return Ok(());
    }
    let root = download_root(files_dir);
    if !root.is_dir() {
        return Ok(());
    }
    let mut prefix = String::from("kavita-");
    prefix.push_str(server_id);
    prefix.push('-');
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        remove_download_path(&root, name)?;
    }
    Ok(())
}

fn remove_download_dirs(files_dir: &Path, ids: &[String]) -> Result<(), SessionError> {
    let root = download_root(files_dir);
    for id in ids {
        remove_download_path(&root, id)?;
    }
    Ok(())
}

fn remove_download_path(root: &Path, name: &str) -> Result<(), SessionError> {
    if !safe_segment(name) {
        return Ok(());
    }
    let path = root.join(name);
    if path.parent() != Some(root) {
        return Ok(());
    }
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::remove_file(&path)?,
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(&path)?,
        Ok(_) => fs::remove_file(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains('\0')
        && value != "."
        && value != ".."
}

fn adopt(
    db: &folio_storage::Database,
    store: &SecretStore,
    old: StoredServer,
) -> Result<String, SessionError> {
    let secret = old.secret_ref.ok_or(SessionError::Server)?;
    let name = if old.display_name.is_empty() {
        "Kavita".to_owned()
    } else {
        old.display_name
    };
    validate_account(&name, &old.url, &secret)?;
    let id = new_id()?;
    store.write(SECRET_KIND, &id, &secret)?;
    let moved = swap_server(
        db,
        &old.id,
        StoredServer {
            // clone: the swapped row keeps the id after this function returns
            id: id.clone(),
            kind: SECRET_KIND.to_owned(),
            url: old.url,
            display_name: name,
            secret_ref: Some(secret_reference(&id)),
        },
    )?;
    if !moved {
        // Another caller moved the row first; this key file belongs to no row.
        store.delete(SECRET_KIND, &id)?;
        return Err(SessionError::Server);
    }
    Ok(id)
}

fn raw_secret(secret: Option<&str>) -> bool {
    match secret {
        Some(value) => {
            !value.is_empty() && !value.starts_with("keyring:") && !value.starts_with("secret:")
        }
        None => false,
    }
}

/// Read the Auth Key. A missing file is an error, not an anonymous session.
///
/// # Errors
///
/// Returns an error when the file is missing or the id is empty.
pub fn read_key(store: &SecretStore, server_id: &str) -> Result<String, SessionError> {
    Ok(store.read(SECRET_KIND, server_id)?)
}

/// Publish `snapshot` as one generation, unless the server was changed since the fetch.
///
/// Each section in `snapshot.gaps.stale` keeps the rows of the previous published generation.
/// Returns the gaps, so the caller can say what was not refreshed.
///
/// # Errors
///
/// Returns [`SessionError::Changed`] when the saved row no longer has `base` and `api_key`, or
/// an error when a row cannot be stored. The previous generation stays visible either way.
pub fn publish_mirror(
    db: &folio_storage::Database,
    store: &SecretStore,
    server_id: &str,
    base: &str,
    api_key: &str,
    snapshot: MirrorSnapshot,
) -> Result<MirrorGaps, SessionError> {
    let mutated = kavita_mirror_mutated(db, server_id)?;
    publish_mirror_at(db, store, server_id, base, api_key, mutated, snapshot)
}

fn publish_mirror_at(
    db: &folio_storage::Database,
    store: &SecretStore,
    server_id: &str,
    base: &str,
    api_key: &str,
    expected_mutated: i64,
    snapshot: MirrorSnapshot,
) -> Result<MirrorGaps, SessionError> {
    let gaps = snapshot.gaps.clone();
    let published = publish_kavita_mirror(
        db,
        server_id,
        mirror_rows(snapshot),
        expected_mutated,
        same_account(store, base, api_key),
    )?;
    if published {
        Ok(gaps)
    } else {
        Err(SessionError::Changed)
    }
}

/// The check a store of fetched rows runs on the saved row, inside the same storage job: it is
/// still Kavita at `base`, and its Auth Key file still holds `api_key`. [`replace_kavita`] writes
/// the key before it drops the mirror, so a key change is seen here before or after, never
/// between.
fn same_account(
    store: &SecretStore,
    base: &str,
    api_key: &str,
) -> impl FnOnce(&StoredServer) -> bool + Send + 'static {
    let store = store.clone();
    let base = base.to_owned();
    let api_key = api_key.to_owned();
    move |row| {
        row.kind == SECRET_KIND
            && row.url == base
            && store
                .read(SECRET_KIND, &row.id)
                .is_ok_and(|key| key == api_key)
    }
}

fn mirror_rows(snapshot: MirrorSnapshot) -> KavitaMirror {
    KavitaMirror {
        libraries: snapshot
            .libraries
            .into_iter()
            .map(|library| KavitaLibraryRow {
                library_id: library.id,
                name: library.name,
            })
            .collect(),
        series: snapshot
            .series
            .into_iter()
            .map(|series| KavitaSeriesRow {
                series_id: series.id,
                name: series.name,
                format: series.format,
                library_id: series.library_id,
            })
            .collect(),
        home: snapshot
            .home
            .into_iter()
            .map(|stream| KavitaHomeRow {
                stream_id: stream.id,
                name: stream.name,
                stream_type: stream.stream_type,
                visible: stream.visible,
                sort_order: stream.order,
            })
            .collect(),
        collections: named_rows(snapshot.collections),
        reading_lists: named_rows(snapshot.reading_lists),
        filters: named_rows(snapshot.filters),
        wants: snapshot.wants,
        bookmarks: snapshot
            .bookmarks
            .into_iter()
            .map(|bookmark| KavitaBookmarkRow {
                bookmark_id: bookmark.id,
                series_id: bookmark.series_id,
                chapter_id: bookmark.chapter_id,
                page: bookmark.page,
            })
            .collect(),
        shelves: vec![
            (ON_DECK.to_owned(), snapshot.on_deck),
            (RECENTLY_ADDED.to_owned(), snapshot.recently_added),
            (RECENTLY_UPDATED.to_owned(), snapshot.recently_updated),
        ],
        keep: snapshot
            .gaps
            .stale
            .iter()
            .map(|section| keep_of(*section))
            .collect(),
    }
}

fn keep_of(section: MirrorSection) -> KavitaKeep {
    match section {
        MirrorSection::Home => KavitaKeep::Home,
        MirrorSection::OnDeck => KavitaKeep::Shelf(ON_DECK.to_owned()),
        MirrorSection::RecentlyAdded => KavitaKeep::Shelf(RECENTLY_ADDED.to_owned()),
        MirrorSection::RecentlyUpdated => KavitaKeep::Shelf(RECENTLY_UPDATED.to_owned()),
        MirrorSection::Collections => KavitaKeep::Collections,
        MirrorSection::ReadingLists => KavitaKeep::ReadingLists,
        MirrorSection::Filters => KavitaKeep::Filters,
        MirrorSection::WantToRead => KavitaKeep::Wants,
        MirrorSection::Bookmarks => KavitaKeep::Bookmarks,
    }
}

fn named_rows(rows: Vec<KavitaNamed>) -> Vec<KavitaNamedRow> {
    rows.into_iter()
        .map(|row| KavitaNamedRow {
            id: row.id,
            name: row.name,
            filter: row.filter,
        })
        .collect()
}

/// Fetch the mirror and publish it. A failed fetch does not publish. Returns the replies that
/// could not be read but did not stop the sync. After a successful publish, download rows whose
/// series is not in the new list are deleted, and their folders under `files_dir` go with them.
///
/// `kavita_generation.mutated` is read before the HTTP fetch so a one-series hide during the
/// fetch refuses this publish.
///
/// # Errors
///
/// Returns an error when the server cannot be reached, refuses the key, or sends a library or
/// series list that cannot be read, and [`SessionError::Changed`] when the server was edited or
/// removed during the fetch.
pub fn fetch_and_publish(
    db: &folio_storage::Database,
    store: &SecretStore,
    files_dir: &Path,
    server_id: &str,
    base: &str,
    api_key: &str,
    allow_invalid_certs: bool,
) -> Result<MirrorGaps, SessionError> {
    let mutated = kavita_mirror_mutated(db, server_id)?;
    let snapshot = fetch_mirror(base, api_key, allow_invalid_certs)?;
    let gaps = publish_mirror_at(db, store, server_id, base, api_key, mutated, snapshot)?;
    let gone = drop_orphan_kavita_downloads(db, server_id)?;
    remove_download_dirs(files_dir, &gone)?;
    Ok(gaps)
}

/// Download one series cover. The bytes are not a catalog row.
///
/// # Errors
///
/// Returns an error when the cover cannot be fetched.
pub fn fetch_cover(
    base: &str,
    api_key: &str,
    series_id: i64,
    allow_invalid_certs: bool,
) -> Result<Vec<u8>, SessionError> {
    let request = cover_request(base, api_key, series_id)?;
    Ok(execute(&request, allow_invalid_certs)?)
}

/// Fetch one page. `kind` is `epub`, `pdf`, or anything else for an image page.
///
/// # Errors
///
/// Returns an error when the page cannot be fetched.
pub fn fetch_page(
    base: &str,
    api_key: &str,
    kind: &str,
    chapter_id: i64,
    page: i64,
    allow_invalid_certs: bool,
) -> Result<Vec<u8>, SessionError> {
    let request = page_request(base, api_key, kind, chapter_id, page)?;
    Ok(execute(&request, allow_invalid_certs)?)
}

/// `downloaded` is a finished chapter on disk. Offline and not downloaded does not open.
pub fn chapter_can_open(downloaded: bool, online: bool) -> bool {
    can_open(downloaded, online)
}

/// Where the reader stopped. This is an outbox row, not a catalog item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageStop {
    /// Series id.
    pub series_id: i64,
    /// Volume id.
    pub volume_id: i64,
    /// Library id.
    pub library_id: i64,
    /// Chapter id.
    pub chapter_id: i64,
    /// Zero-based page.
    pub page: i64,
    /// Pages in the chapter. Zero means the count is unknown.
    pub page_count: i64,
}

/// Record a page in the outbox. This does not write a catalog row and does not claim a send.
///
/// # Errors
///
/// Returns an error when the locator cannot be stored.
pub fn record_page(
    db: &folio_storage::Database,
    server_id: &str,
    stop: PageStop,
) -> Result<(), SessionError> {
    let updated_at = now_ms()?;
    enqueue_kavita_progress(
        db,
        KavitaOutboxRow {
            server_id: server_id.to_owned(),
            chapter_id: stop.chapter_id,
            series_id: stop.series_id,
            volume_id: stop.volume_id,
            library_id: stop.library_id,
            locator: page_locator(stop.page),
            complete: stop.page_count > 0 && stop.page.saturating_add(1) >= stop.page_count,
            updated_at,
        },
    )?;
    Ok(())
}

/// Queue a read or unread mark. `scope` is `chapter`, `volume`, or `series`.
///
/// # Errors
///
/// Returns an error when the scope is not one of those three.
pub fn record_mark(
    db: &folio_storage::Database,
    server_id: &str,
    scope: &str,
    scope_id: i64,
    series_id: i64,
    volume_id: i64,
    read: bool,
) -> Result<(), SessionError> {
    enqueue_kavita_mark(
        db,
        KavitaMarkRow {
            server_id: server_id.to_owned(),
            scope: scope.to_owned(),
            scope_id,
            series_id,
            volume_id,
            read,
            updated_at: now_ms()?,
        },
    )?;
    Ok(())
}

/// Store a pulled locator only when that chapter has no outbox row.
///
/// # Errors
///
/// Returns an error when the writer cannot commit.
pub fn pull_progress(
    db: &folio_storage::Database,
    server_id: &str,
    chapter_id: i64,
    locator: &str,
    updated_at: i64,
) -> Result<(), SessionError> {
    Ok(apply_kavita_pull(
        db, server_id, chapter_id, locator, updated_at,
    )?)
}

/// Push the outbox and marks. `true` means at least one HTTP call succeeded and none failed.
///
/// A transport error leaves the rows and returns `false` only when nothing succeeded.
/// One success followed by a failure returns an error after acknowledging the successes,
/// so the caller must not describe the whole sync as sent.
///
/// # Errors
///
/// Returns an error when a request cannot be built. A refused connection is `Ok(false)`
/// when no row was acknowledged.
pub fn push_pending(
    db: &folio_storage::Database,
    server_id: &str,
    base: &str,
    api_key: &str,
    allow_invalid_certs: bool,
) -> Result<bool, SessionError> {
    push_with(db, server_id, base, api_key, |request| {
        execute(request, allow_invalid_certs).map(|_| ())
    })
}

/// Same as [`push_pending`] with a caller-supplied send. Tests use this so they do not open a socket.
///
/// # Errors
///
/// Returns an error when a request cannot be built or a send fails after an earlier success.
pub fn push_with<F>(
    db: &folio_storage::Database,
    server_id: &str,
    base: &str,
    api_key: &str,
    mut send: F,
) -> Result<bool, SessionError>
where
    F: FnMut(&KavitaRequest) -> Result<(), ServerError>,
{
    let mut delivered = false;
    for row in kavita_outbox(db, server_id)? {
        let Some(page) = page_from_locator(&row.locator) else {
            continue;
        };
        let updated = rfc3339(row.updated_at);
        let request = progress_request(
            base,
            api_key,
            &KavitaProgress {
                series_id: row.series_id,
                volume_id: row.volume_id,
                chapter_id: row.chapter_id,
                library_id: row.library_id,
                page,
                updated_rfc3339: &updated,
            },
        )?;
        match send(&request) {
            Ok(()) => {
                acknowledge_kavita_progress(db, &row)?;
                delivered = true;
            }
            Err(_) if delivered => return Err(SessionError::Remote(ServerError::Transport)),
            Err(_) => return Ok(false),
        }
    }
    for row in kavita_marks(db, server_id)? {
        let request = mark_request(base, api_key, &row)?;
        match send(&request) {
            Ok(()) => {
                acknowledge_kavita_mark(db, &row)?;
                delivered = true;
            }
            Err(_) if delivered => return Err(SessionError::Remote(ServerError::Transport)),
            Err(_) => return Ok(false),
        }
    }
    Ok(delivered)
}

/// Queue a chapter download. The stored URL has no Auth Key.
///
/// # Errors
///
/// Returns an error when the URL is not on `origin`.
pub fn queue_chapter(
    db: &folio_storage::Database,
    server_id: &str,
    origin: &str,
    chapter_id: i64,
    series_id: i64,
    kind: &str,
) -> Result<(), SessionError> {
    let url = chapter_url(origin, chapter_id, kind)?;
    if !same_origin(origin, &url) {
        return Err(SessionError::Server);
    }
    let id = download_id(server_id, chapter_id);
    enqueue_kavita_download(db, &id, server_id, &url, series_id)?;
    Ok(())
}

/// Reset rows left `running` by a previous process.
///
/// # Errors
///
/// Returns an error when a partial directory cannot be removed.
pub fn recover_downloads(db: &folio_storage::Database, root: &Path) -> Result<(), SessionError> {
    Ok(recover(db, root)?)
}

/// Cancel the file in progress and queue it from the first byte.
///
/// # Errors
///
/// Returns an error when a partial directory cannot be removed.
pub fn cancel_downloads(db: &folio_storage::Database, root: &Path) -> Result<(), SessionError> {
    Ok(leave_foreground(db, root)?)
}

/// Whether a queued download may use the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadPolicy {
    /// Accept a certificate the platform would reject.
    pub allow_invalid_certs: bool,
    /// The link is not metered. A metered link refuses the download.
    pub unmetered: bool,
    /// The reader is in the foreground. Leaving it cancels the partial file.
    pub foreground: bool,
}

/// Fetch queued chapters into `root`. A metered network refuses. The catalog is not written.
///
/// # Errors
///
/// Returns an error when the network is metered, the origin does not match, or a page is missing.
pub fn run_downloads(
    db: &folio_storage::Database,
    root: &Path,
    origin: &str,
    api_key: &str,
    kind_of: impl Fn(&str) -> Option<(String, i64)>,
    policy: DownloadPolicy,
) -> Result<(), SessionError> {
    run_kavita(
        db,
        root,
        origin,
        policy.unmetered,
        policy.foreground,
        |row, directory| {
            let Some((kind, pages)) = kind_of(&row.id) else {
                return Err(IoError::other("unknown chapter"));
            };
            let chapter_id =
                chapter_from_download_id(&row.id).ok_or_else(|| IoError::other("id"))?;
            store_chapter(
                directory,
                origin,
                api_key,
                &kind,
                chapter_id,
                pages,
                policy.allow_invalid_certs,
            )
        },
    )?;
    Ok(())
}

/// Whether `directory` has a finished chapter of `page_count` pages.
pub fn directory_complete(directory: &Path, page_count: i64) -> bool {
    if !directory.join("complete").is_file() {
        return false;
    }
    let stored = match fs::read_to_string(directory.join("pages.txt")) {
        Ok(text) => text,
        Err(_) => return false,
    };
    let mut pages = Vec::new();
    for line in stored.lines() {
        let mut parts = line.split(' ');
        let Some(index) = parts.next().and_then(|text| text.parse::<i64>().ok()) else {
            return false;
        };
        let Some(expected) = parts.next().and_then(|text| text.parse::<u64>().ok()) else {
            return false;
        };
        let Some(images) = parts.next().and_then(|text| text.parse::<u64>().ok()) else {
            return false;
        };
        pages.push(StoredPage {
            index,
            images_expected: expected,
            images_stored: images,
        });
    }
    download_complete(page_count, &pages)
}

/// Read one stored page. HTML is UTF-8. Images and PDF are raw bytes.
pub fn stored_page(directory: &Path, kind: &str, page: i64) -> Option<Vec<u8>> {
    let path = if kind == "epub" {
        directory.join(format!("page-{page}.html"))
    } else if kind == "pdf" {
        directory.join("chapter.pdf")
    } else {
        directory.join(format!("page-{page}.bin"))
    };
    fs::read(path).ok()
}

fn store_chapter(
    directory: &Path,
    origin: &str,
    api_key: &str,
    kind: &str,
    chapter_id: i64,
    pages: i64,
    allow_invalid_certs: bool,
) -> Result<bool, IoError> {
    if pages <= 0 {
        return Ok(false);
    }
    let mut lines = String::new();
    if kind == "pdf" {
        let request =
            pdf_request(origin, api_key, chapter_id).map_err(|_| IoError::other("url"))?;
        let bytes = execute(&request, allow_invalid_certs).map_err(|_| IoError::other("fetch"))?;
        fs::write(directory.join("chapter.pdf"), &bytes)?;
        let mut stored = Vec::new();
        for page in 0..pages {
            stored.push((page, 0, 0));
            lines.push_str(&format!("{page} 0 0\n"));
        }
        return finish_chapter(directory, pages, &lines, &stored);
    }
    let mut stored = Vec::new();
    for page in 0..pages {
        if kind == "epub" {
            let request = epub_page_request(origin, api_key, chapter_id, page)
                .map_err(|_| IoError::other("url"))?;
            let bytes =
                execute(&request, allow_invalid_certs).map_err(|_| IoError::other("fetch"))?;
            let html = String::from_utf8(bytes).map_err(|_| IoError::other("utf8"))?;
            fs::write(directory.join(format!("page-{page}.html")), &html)?;
            let sources = remote_image_sources(&html);
            let mut images = 0u64;
            for (index, source) in sources.iter().enumerate() {
                let Some(absolute) = absolute_url(origin, source) else {
                    return Ok(false);
                };
                if !same_origin(origin, &absolute) {
                    return Ok(false);
                }
                let request = authed_get(&absolute, api_key).map_err(|_| IoError::other("url"))?;
                let image =
                    execute(&request, allow_invalid_certs).map_err(|_| IoError::other("fetch"))?;
                let image_index = u64::try_from(index).unwrap_or(0);
                fs::write(
                    directory.join(format!("page-{page}-img-{image_index}")),
                    image,
                )?;
                images = images.saturating_add(1);
            }
            let expected = u64::try_from(sources.len()).unwrap_or(0);
            stored.push((page, expected, images));
            lines.push_str(&format!("{page} {expected} {images}\n"));
        } else {
            let request = image_page_request(origin, api_key, chapter_id, page)
                .map_err(|_| IoError::other("url"))?;
            let bytes =
                execute(&request, allow_invalid_certs).map_err(|_| IoError::other("fetch"))?;
            fs::write(directory.join(format!("page-{page}.bin")), bytes)?;
            stored.push((page, 0, 0));
            lines.push_str(&format!("{page} 0 0\n"));
        }
    }
    finish_chapter(directory, pages, &lines, &stored)
}

fn finish_chapter(
    directory: &Path,
    pages: i64,
    lines: &str,
    stored: &[(i64, u64, u64)],
) -> Result<bool, IoError> {
    let pages_stored = stored
        .iter()
        .map(|(index, expected, images)| StoredPage {
            index: *index,
            images_expected: *expected,
            images_stored: *images,
        })
        .collect::<Vec<_>>();
    if !download_complete(pages, &pages_stored) {
        return Ok(false);
    }
    fs::write(directory.join("pages.txt"), lines)?;
    fs::write(directory.join("complete"), b"")?;
    Ok(true)
}

fn fetch_mirror(
    base: &str,
    api_key: &str,
    allow_invalid_certs: bool,
) -> Result<MirrorSnapshot, SessionError> {
    mirror_with(base, api_key, |request| {
        text_of(request, allow_invalid_certs)
    })
}

/// Build a mirror from the replies `fetch` returns. Every failure names its [`MirrorStep`].
/// Chapters are not read here; [`fetch_chapters`] reads them when a series is opened.
///
/// Only an unreadable reply ([`ServerError::Feed`]) is tolerated, and only outside the library
/// and series lists: a refusal or a lost connection means the server stopped answering, so the
/// whole sync stops and the previous generation stays visible.
fn mirror_with<F>(base: &str, api_key: &str, mut fetch: F) -> Result<MirrorSnapshot, SessionError>
where
    F: FnMut(&KavitaRequest) -> Result<String, SessionError>,
{
    let libraries = read(
        &mut fetch,
        MirrorStep::List(MirrorList::Libraries),
        &libraries_request(base, api_key)?,
        parse_libraries,
    )?;
    let series = read_pages(
        &mut fetch,
        MirrorStep::List(MirrorList::Series),
        |page| all_series_request(base, api_key, page),
        parse_series,
        |row| row.id,
    )?;
    let mut gaps = MirrorGaps::default();
    let on_deck = optional(
        &mut fetch,
        &mut gaps,
        MirrorSection::OnDeck,
        &on_deck_request(base, api_key)?,
        parse_series_ids,
    )?;
    let recently_added = optional(
        &mut fetch,
        &mut gaps,
        MirrorSection::RecentlyAdded,
        &recently_added_request(base, api_key)?,
        parse_series_ids,
    )?;
    let recently_updated = optional(
        &mut fetch,
        &mut gaps,
        MirrorSection::RecentlyUpdated,
        &recently_updated_request(base, api_key)?,
        parse_grouped_series_ids,
    )?;
    let home = optional(
        &mut fetch,
        &mut gaps,
        MirrorSection::Home,
        &dashboard_request(base, api_key)?,
        parse_dashboard,
    )?;
    let collections = optional(
        &mut fetch,
        &mut gaps,
        MirrorSection::Collections,
        &collections_request(base, api_key)?,
        parse_titled,
    )?;
    let reading_lists = optional(
        &mut fetch,
        &mut gaps,
        MirrorSection::ReadingLists,
        &reading_lists_request(base, api_key)?,
        parse_titled,
    )?;
    let filters = optional(
        &mut fetch,
        &mut gaps,
        MirrorSection::Filters,
        &filters_request(base, api_key)?,
        parse_filters,
    )?;
    let wants = tolerate(
        &mut gaps,
        MirrorSection::WantToRead,
        read_pages(
            &mut fetch,
            MirrorStep::Section(MirrorSection::WantToRead),
            |page| want_to_read_request(base, api_key, page),
            parse_series_ids,
            |id| *id,
        ),
    )?;
    let bookmarks = optional(
        &mut fetch,
        &mut gaps,
        MirrorSection::Bookmarks,
        &bookmarks_request(base, api_key)?,
        parse_bookmarks,
    )?;
    Ok(MirrorSnapshot {
        libraries,
        series,
        home,
        collections,
        reading_lists,
        filters,
        wants,
        bookmarks,
        on_deck,
        recently_added,
        recently_updated,
        gaps,
    })
}

fn read<F, T>(
    fetch: &mut F,
    step: MirrorStep,
    request: &KavitaRequest,
    parse: impl FnOnce(&str) -> Result<T, ServerError>,
) -> Result<T, SessionError>
where
    F: FnMut(&KavitaRequest) -> Result<String, SessionError>,
{
    let named = |error: ServerError| SessionError::Mirror { step, error };
    let text = fetch(request).map_err(|error| match error {
        SessionError::Remote(error) => named(error),
        other => other,
    })?;
    parse(&text).map_err(named)
}

fn optional<F, T: Default>(
    fetch: &mut F,
    gaps: &mut MirrorGaps,
    section: MirrorSection,
    request: &KavitaRequest,
    parse: impl FnOnce(&str) -> Result<T, ServerError>,
) -> Result<T, SessionError>
where
    F: FnMut(&KavitaRequest) -> Result<String, SessionError>,
{
    tolerate(
        gaps,
        section,
        read(fetch, MirrorStep::Section(section), request, parse),
    )
}

/// An unreadable optional section becomes a gap instead of an error.
fn tolerate<T: Default>(
    gaps: &mut MirrorGaps,
    section: MirrorSection,
    result: Result<T, SessionError>,
) -> Result<T, SessionError> {
    match result {
        Err(SessionError::Mirror {
            error: ServerError::Feed,
            ..
        }) => {
            gaps.stale.push(section);
            Ok(T::default())
        }
        other => other,
    }
}

/// Read a paged list from page 1 until a page holds other than [`KAVITA_PAGE`] raw rows or
/// adds no new id; the second rule stops a server that ignores `PageSize` from being read for
/// ever.
fn read_pages<F, T>(
    fetch: &mut F,
    step: MirrorStep,
    request: impl Fn(u32) -> Result<KavitaRequest, ServerError>,
    parse: impl Fn(&str) -> Result<Vec<T>, ServerError>,
    id: impl Fn(&T) -> i64,
) -> Result<Vec<T>, SessionError>
where
    F: FnMut(&KavitaRequest) -> Result<String, SessionError>,
{
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for page in 1.. {
        let (count, found) = read(fetch, step, &request(page)?, |text| {
            Ok((page_len(text)?, parse(text)?))
        })?;
        let before = rows.len();
        rows.extend(found.into_iter().filter(|row| seen.insert(id(row))));
        if count != KAVITA_PAGE || rows.len() == before {
            break;
        }
    }
    Ok(rows)
}

/// Read the chapters of `series_id` from Kavita and replace the stored ones. A failure leaves
/// the stored chapters as they were.
///
/// # Errors
///
/// Returns an error when the server cannot be reached, refuses the key, or sends a reply that
/// cannot be read, when the rows cannot be stored, or [`SessionError::Changed`] when the server
/// was edited or removed during the fetch.
pub fn fetch_chapters(
    db: &folio_storage::Database,
    store: &SecretStore,
    server_id: &str,
    base: &str,
    api_key: &str,
    allow_invalid_certs: bool,
    series_id: i64,
) -> Result<(), SessionError> {
    refresh_chapters_with(db, store, server_id, base, api_key, series_id, |request| {
        text_of(request, allow_invalid_certs)
    })
}

/// Fetch volumes for `series_id`. HTTP 401/403 drops that series and returns [`SessionError::Hidden`].
///
/// # Errors
///
/// Returns [`SessionError::Hidden`] when Kavita refuses the series, [`SessionError::Changed`]
/// when the account moved, or a remote error when the volumes request fails for any other reason.
pub fn admit_kavita_series(
    db: &folio_storage::Database,
    store: &SecretStore,
    files_dir: &Path,
    server_id: &str,
    origin: (&str, &str),
    allow_invalid_certs: bool,
    series_id: i64,
) -> Result<(), SessionError> {
    admit_kavita_series_with(
        db,
        store,
        files_dir,
        server_id,
        origin,
        series_id,
        |request| text_of(request, allow_invalid_certs),
    )
}

fn admit_kavita_series_with<F>(
    db: &folio_storage::Database,
    store: &SecretStore,
    files_dir: &Path,
    server_id: &str,
    origin: (&str, &str),
    series_id: i64,
    fetch: F,
) -> Result<(), SessionError>
where
    F: FnMut(&KavitaRequest) -> Result<String, SessionError>,
{
    let (base, api_key) = origin;
    match refresh_chapters_with(db, store, server_id, base, api_key, series_id, fetch) {
        Ok(()) => Ok(()),
        Err(error) if volumes_refused(&error) => {
            hide_series(db, store, files_dir, server_id, base, api_key, series_id)?;
            Err(SessionError::Hidden)
        }
        Err(error) => Err(error),
    }
}

fn volumes_refused(error: &SessionError) -> bool {
    matches!(
        error,
        SessionError::Remote(ServerError::Refused)
            | SessionError::Mirror {
                error: ServerError::Refused,
                ..
            }
    )
}

fn hide_series(
    db: &folio_storage::Database,
    store: &SecretStore,
    files_dir: &Path,
    server_id: &str,
    base: &str,
    api_key: &str,
    series_id: i64,
) -> Result<(), SessionError> {
    match drop_kavita_series(db, server_id, series_id, same_account(store, base, api_key))? {
        Some(ids) => remove_download_dirs(files_dir, &ids),
        None => Err(SessionError::Changed),
    }
}

fn refresh_chapters_with<F>(
    db: &folio_storage::Database,
    store: &SecretStore,
    server_id: &str,
    base: &str,
    api_key: &str,
    series_id: i64,
    mut fetch: F,
) -> Result<(), SessionError>
where
    F: FnMut(&KavitaRequest) -> Result<String, SessionError>,
{
    let chapters = read(
        &mut fetch,
        MirrorStep::Chapters,
        &volumes_request(base, api_key, series_id)?,
        |text| parse_volumes(text, series_id),
    )?;
    let rows = chapters
        .into_iter()
        .map(|chapter| KavitaChapterRow {
            chapter_id: chapter.id,
            series_id: chapter.series_id,
            volume_id: chapter.volume_id,
            name: chapter.name,
            pages: chapter.pages,
        })
        .collect();
    if replace_kavita_chapters(
        db,
        server_id,
        series_id,
        rows,
        same_account(store, base, api_key),
    )? {
        Ok(())
    } else {
        Err(SessionError::Changed)
    }
}

fn text_of(request: &KavitaRequest, allow_invalid_certs: bool) -> Result<String, SessionError> {
    let bytes = execute(request, allow_invalid_certs)?;
    String::from_utf8(bytes).map_err(|_| SessionError::Remote(ServerError::Feed))
}

fn mark_request(
    base: &str,
    api_key: &str,
    row: &KavitaMarkRow,
) -> Result<KavitaRequest, ServerError> {
    match row.scope.as_str() {
        "chapter" => mark_chapter_request(base, api_key, row.series_id, row.scope_id, row.read),
        "volume" => mark_volume_request(base, api_key, row.series_id, row.volume_id, row.read),
        "series" => mark_series_request(base, api_key, row.series_id, row.read),
        _ => Err(ServerError::Feed),
    }
}

fn page_request(
    base: &str,
    api_key: &str,
    kind: &str,
    chapter_id: i64,
    page: i64,
) -> Result<KavitaRequest, ServerError> {
    if kind == "epub" {
        epub_page_request(base, api_key, chapter_id, page)
    } else if kind == "pdf" {
        pdf_request(base, api_key, chapter_id)
    } else {
        image_page_request(base, api_key, chapter_id, page)
    }
}

fn chapter_url(origin: &str, chapter_id: i64, kind: &str) -> Result<String, SessionError> {
    let request = page_request(origin, "placeholder", kind, chapter_id, 0)?;
    let mut url = request.url().clone();
    let pairs = url
        .query_pairs()
        .filter(|(key, _)| key != "apiKey")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    if !pairs.is_empty() {
        let mut query = url.query_pairs_mut();
        for (key, value) in &pairs {
            query.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

/// `page:<n>`.
pub fn page_locator(page: i64) -> String {
    let mut locator = String::from("page:");
    locator.push_str(&page.to_string());
    locator
}

/// Page index from `page:<n>`.
pub fn page_from_locator(locator: &str) -> Option<i64> {
    locator
        .strip_prefix("page:")
        .and_then(|text| text.parse().ok())
}

/// Secret-store reference stored in the database.
pub fn secret_reference(server_id: &str) -> String {
    let mut reference = String::from("secret:kavita:");
    reference.push_str(server_id);
    reference
}

/// Directory for one chapter download.
pub fn chapter_directory(root: &Path, server_id: &str, chapter_id: i64) -> PathBuf {
    root.join(download_id(server_id, chapter_id))
}

/// Download root beside the database.
pub fn download_root(files_dir: &Path) -> PathBuf {
    files_dir.join("kavita-downloads")
}

fn download_id(server_id: &str, chapter_id: i64) -> String {
    let mut id = String::from("kavita-");
    id.push_str(server_id);
    id.push('-');
    id.push_str(&chapter_id.to_string());
    id
}

fn chapter_from_download_id(id: &str) -> Option<i64> {
    id.rsplit_once('-')?.1.parse().ok()
}

fn validate_account(name: &str, url: &str, api_key: &str) -> Result<(), SessionError> {
    if name.is_empty() || url.contains("password=") {
        return Err(SessionError::Server);
    }
    libraries_request(url, api_key)?;
    Ok(())
}

fn kavita_server(db: &folio_storage::Database, id: &str) -> Result<StoredServer, SessionError> {
    servers(db)?
        .into_iter()
        .find(|server| server.id == id && server.kind == SECRET_KIND)
        .ok_or(SessionError::Server)
}

fn new_id() -> Result<String, SessionError> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| SessionError::Server)?;
    Ok(hex_encode(&bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let high = usize::from(byte >> 4);
        let low = usize::from(byte & 0x0f);
        out.push(char::from(HEX[high]));
        out.push(char::from(HEX[low]));
    }
    out
}

fn now_ms() -> Result<i64, SessionError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SessionError::Clock)?;
    i64::try_from(elapsed.as_millis()).map_err(|_| SessionError::Clock)
}

fn rfc3339(ms: i64) -> String {
    if ms < 0 {
        return "1970-01-01T00:00:00.000Z".to_owned();
    }
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let tod = secs.rem_euclid(86_400);
    let hour = tod.div_euclid(3600);
    let minute = tod.rem_euclid(3600).div_euclid(60);
    let second = tod.rem_euclid(60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

fn remote_image_sources(html: &str) -> Vec<String> {
    let mut sources = Vec::new();
    let bytes = html.as_bytes();
    let folded = html.to_ascii_lowercase();
    let mut index = 0;
    while index < bytes.len() {
        let Some(relative) = folded.get(index..).and_then(|rest| rest.find("src=")) else {
            break;
        };
        index = index.saturating_add(relative).saturating_add(4);
        let quote = bytes.get(index).copied();
        if !matches!(quote, Some(b'"' | b'\'')) {
            continue;
        }
        index = index.saturating_add(1);
        let start = index;
        while index < bytes.len() && bytes.get(index) != Some(&quote.unwrap_or(b'"')) {
            index = index.saturating_add(1);
        }
        let value = &html[start..index];
        if value.starts_with("data:") || value.is_empty() {
            continue;
        }
        sources.push(value.to_owned());
        index = index.saturating_add(1);
    }
    sources
}

fn absolute_url(origin: &str, source: &str) -> Option<String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return Some(source.to_owned());
    }
    url::Url::parse(origin)
        .ok()?
        .join(source)
        .ok()
        .map(|url| url.to_string())
}

/// Write `bytes` as a data URL. The name selects the media type.
pub fn bytes_data_url(name: &str, bytes: &[u8]) -> String {
    crate::session::page_data_url(name, bytes)
}

/// Covers still to fetch for the series on screen: the first `limit` distinct ids in screen
/// order, minus those already in `cached` (a failed fetch is cached as an empty string). The
/// cap is on the cards, not on the misses, so a loader that calls this after every batch stops
/// once the first `limit` cards are covered instead of walking the whole list.
pub fn covers_to_fetch(shown: &[i64], cached: &BTreeMap<i64, String>, limit: usize) -> Vec<i64> {
    let mut seen = HashSet::new();
    shown
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .take(limit)
        .filter(|id| !cached.contains_key(id))
        .collect()
}

/// One page of a list of cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListPage {
    /// The page shown, counted from zero and never past the last page.
    pub index: usize,
    /// How many pages the list has; an empty list has one.
    pub pages: usize,
    /// First item on the page.
    pub start: usize,
    /// One past the last item on the page.
    pub end: usize,
    /// Items in the whole list.
    pub total: usize,
}

/// The page at `index` of a `total`-item list shown `size` items at a time. An index past the
/// end lands on the last page, so a filter that shrinks the list never leaves an empty page.
pub fn list_page(total: usize, index: usize, size: usize) -> ListPage {
    let size = size.max(1);
    let pages = total.div_ceil(size).max(1);
    let index = index.min(pages.saturating_sub(1));
    let start = index.saturating_mul(size).min(total);
    let end = start.saturating_add(size).min(total);
    ListPage {
        index,
        pages,
        start,
        end,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        add_kavita, admit_kavita_series_with, chapter_can_open, chapter_directory,
        connection_failure, directory_complete, download_root, forget_server, mirror_with,
        page_from_locator, probe_kavita, publish_mirror, pull_progress, push_with, record_page,
        refresh_chapters_with, refresh_status, rehome_raw_opds, rename_kavita, replace_kavita,
        secret_reference, update_kavita, MirrorGaps, MirrorList, MirrorSection, MirrorSnapshot,
        MirrorStep, PageStop,
    };
    use crate::session::{open_database, SessionError};
    use folio_platform::SecretStore;
    use folio_server::{KavitaBookmark, KavitaNamed, KavitaRequest, KavitaSeries, ServerError};
    use folio_storage::{
        enqueue_kavita_download, export_backup, kavita_outbox, kavita_pending, servers,
        upsert_server, visible_kavita_chapters, visible_kavita_collections, visible_kavita_mirror,
        visible_kavita_series, StoredServer,
    };

    const BASE: &str = "http://kavita.test:5000";

    /// A Kavita server in memory. `replies` overrides the reply for a path, or for a
    /// path and query such as `/api/Series/volumes?seriesId=2`.
    fn fake_kavita(
        replies: Vec<(&'static str, Result<&'static str, ServerError>)>,
    ) -> impl FnMut(&KavitaRequest) -> Result<String, SessionError> {
        move |request| {
            let path = request.url().path().to_owned();
            let full = match request.url().query() {
                Some(query) => format!("{path}?{query}"),
                None => path.clone(),
            };
            if let Some((_, reply)) = replies.iter().find(|(key, _)| *key == full || *key == path) {
                return reply.clone().map(str::to_owned).map_err(SessionError::from);
            }
            let page = request
                .url()
                .query_pairs()
                .find(|(key, _)| key == "PageNumber")
                .map(|(_, value)| value.into_owned());
            if page.as_deref().is_some_and(|page| page != "1") {
                return Ok("[]".to_owned());
            }
            let body = match path.as_str() {
                "/api/Library/libraries" => r#"[{"id":1,"name":"Books"}]"#.to_owned(),
                "/api/Series/all-v2" => r#"[{"id":1,"name":"Wings","libraryId":1,"format":3},
                    {"id":2,"name":"Tide","libraryId":1,"format":1}]"#
                    .to_owned(),
                "/api/Series/on-deck" => r#"[{"id":2,"name":"Tide"}]"#.to_owned(),
                "/api/Series/recently-added-v2" => r#"[{"id":1,"name":"Wings"}]"#.to_owned(),
                "/api/Series/recently-updated-series" => r#"[
                    {"seriesId":2,"seriesName":"Tide","id":7,"chapterId":21},
                    {"seriesId":2,"seriesName":"Tide","id":8,"chapterId":22},
                    {"seriesId":1,"seriesName":"Wings","id":9,"chapterId":11}]"#
                    .to_owned(),
                "/api/Stream/dashboard" => {
                    r#"[{"id":1,"name":"On Deck","streamType":1,"visible":true,"order":0}]"#
                        .to_owned()
                }
                "/api/Collection" => r#"[{"id":3,"title":"Favourites"}]"#.to_owned(),
                "/api/ReadingList/lists" => r#"[{"id":4,"title":"Summer"}]"#.to_owned(),
                "/api/Filter" => r#"[{"id":5,"name":"Unread","filter":"x"}]"#.to_owned(),
                "/api/want-to-read/v2" => r#"[{"id":1,"name":null}]"#.to_owned(),
                "/api/Reader/all-bookmarks" => {
                    r#"[{"id":6,"seriesId":1,"chapterId":11,"page":3}]"#.to_owned()
                }
                "/api/Series/volumes" => {
                    let series = request
                        .url()
                        .query_pairs()
                        .find(|(key, _)| key == "seriesId")
                        .map(|(_, value)| value.into_owned())
                        .unwrap_or_default();
                    format!(
                        r#"[{{"id":10,"chapters":[{{"id":1{series},"title":null,"number":"1","pages":5}}]}}]"#
                    )
                }
                _ => return Err(SessionError::Remote(ServerError::Feed)),
            };
            Ok(body)
        }
    }

    #[test]
    fn every_reply_kavita_sends_is_read() {
        let snapshot = mirror_with(BASE, "raw-api-key", fake_kavita(Vec::new())).expect("mirror");
        assert_eq!(snapshot.gaps, MirrorGaps::default());
        assert_eq!(snapshot.series.len(), 2);
        assert_eq!(snapshot.recently_updated, [2, 1]);
        assert_eq!(snapshot.wants, [1]);
        assert_eq!(snapshot.collections.len(), 1);
        assert_eq!(snapshot.bookmarks.len(), 1);
    }

    #[test]
    fn an_unreadable_section_does_not_stop_the_series() {
        let snapshot = mirror_with(
            BASE,
            "raw-api-key",
            fake_kavita(vec![
                ("/api/Collection", Ok("{")),
                ("/api/Reader/all-bookmarks", Err(ServerError::Feed)),
            ]),
        )
        .expect("mirror");
        assert_eq!(
            snapshot.gaps.stale,
            [MirrorSection::Collections, MirrorSection::Bookmarks]
        );
        assert_eq!(snapshot.series.len(), 2);
        assert!(snapshot.collections.is_empty());
        assert_eq!(snapshot.reading_lists.len(), 1);
    }

    fn series_page(first: i64, count: i64) -> &'static str {
        let rows: Vec<String> = (first..first + count)
            .map(|id| format!(r#"{{"id":{id},"name":"Series {id}","libraryId":1,"format":3}}"#))
            .collect();
        Box::leak(format!("[{}]", rows.join(",")).into_boxed_str())
    }

    fn series_pages_read(
        replies: Vec<(&'static str, Result<&'static str, ServerError>)>,
    ) -> (usize, usize) {
        let mut calls = 0;
        let mut kavita = fake_kavita(replies);
        let snapshot = mirror_with(BASE, "raw-api-key", |request| {
            if request.url().path() == "/api/Series/all-v2" {
                calls += 1;
            }
            kavita(request)
        })
        .expect("mirror");
        (snapshot.series.len(), calls)
    }

    #[test]
    fn the_series_list_is_read_page_by_page() {
        let read = series_pages_read(vec![
            (
                "/api/Series/all-v2?PageNumber=1&PageSize=100",
                Ok(series_page(1, 100)),
            ),
            (
                "/api/Series/all-v2?PageNumber=2&PageSize=100",
                Ok(series_page(101, 1)),
            ),
        ]);
        assert_eq!(read, (101, 2));
    }

    #[test]
    fn a_server_that_ignores_the_page_size_is_not_read_for_ever() {
        assert_eq!(
            series_pages_read(vec![("/api/Series/all-v2", Ok(series_page(1, 100)))]),
            (100, 2)
        );
        assert_eq!(
            series_pages_read(vec![("/api/Series/all-v2", Ok(series_page(1, 150)))]),
            (150, 1)
        );
    }

    #[test]
    fn opening_a_series_replaces_its_chapters() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", BASE, "raw-api-key").expect("add");
        let mut snapshot = MirrorSnapshot::empty();
        snapshot.series.push(KavitaSeries {
            id: 2,
            name: "Tide".to_owned(),
            format: "archive".to_owned(),
            library_id: 1,
        });
        publish_mirror(&db, &store, &id, BASE, "raw-api-key", snapshot).expect("publish");
        refresh_chapters_with(
            &db,
            &store,
            &id,
            BASE,
            "raw-api-key",
            2,
            fake_kavita(Vec::new()),
        )
        .expect("chapters");
        let chapters = visible_kavita_chapters(&db, &id, 2).expect("visible");
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].chapter_id, 12);
        assert_eq!(chapters[0].name, "1");
        let error = refresh_chapters_with(
            &db,
            &store,
            &id,
            BASE,
            "raw-api-key",
            2,
            fake_kavita(vec![("/api/Series/volumes", Err(ServerError::Transport))]),
        )
        .expect_err("offline");
        assert_eq!(
            connection_failure(&error),
            "The server stopped answering while Folio read the chapters."
        );
        assert_eq!(visible_kavita_chapters(&db, &id, 2).expect("kept").len(), 1);
    }

    #[test]
    fn an_unreadable_required_list_is_named() {
        for (path, list) in [
            ("/api/Library/libraries", MirrorList::Libraries),
            ("/api/Series/all-v2", MirrorList::Series),
        ] {
            let error = mirror_with(BASE, "raw-api-key", fake_kavita(vec![(path, Ok("{}"))]))
                .expect_err("required");
            assert!(matches!(
                error,
                SessionError::Mirror {
                    step: MirrorStep::List(found),
                    error: ServerError::Feed,
                } if found == list
            ));
        }
        let unreadable = |list| SessionError::Mirror {
            step: MirrorStep::List(list),
            error: ServerError::Feed,
        };
        assert_eq!(
            connection_failure(&unreadable(MirrorList::Series)),
            "Kavita answered, but the series list could not be read."
        );
        assert_eq!(
            connection_failure(&unreadable(MirrorList::Libraries)),
            "Kavita answered, but the library list could not be read."
        );
    }

    #[test]
    fn a_lost_connection_or_a_refusal_stops_the_sync_and_names_the_step() {
        let cases = [
            (
                "/api/Series/all-v2",
                ServerError::Transport,
                "The server stopped answering while Folio read the series list.",
            ),
            (
                "/api/Series/recently-added-v2",
                ServerError::Transport,
                "The server stopped answering while Folio read Added.",
            ),
            (
                "/api/Filter",
                ServerError::Refused,
                "The server refused the Auth Key.",
            ),
        ];
        for (path, failure, message) in cases {
            let error = mirror_with(BASE, "raw-api-key", fake_kavita(vec![(path, Err(failure))]))
                .expect_err("stopped");
            assert_eq!(connection_failure(&error), message);
        }
        let error = mirror_with(
            BASE,
            "raw-api-key",
            fake_kavita(vec![("/api/Filter", Err(ServerError::Refused))]),
        )
        .expect_err("refused");
        assert!(matches!(
            error,
            SessionError::Mirror {
                step: MirrorStep::Section(MirrorSection::Filters),
                error: ServerError::Refused,
            }
        ));
    }

    #[test]
    fn a_stale_section_keeps_the_last_good_rows() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", BASE, "raw-api-key").expect("add");
        let mut first = MirrorSnapshot::empty();
        first.collections.push(KavitaNamed {
            id: 3,
            name: "Favourites".to_owned(),
            filter: String::new(),
        });
        first.on_deck = vec![7, 5];
        first.bookmarks.push(KavitaBookmark {
            id: 6,
            series_id: 7,
            chapter_id: 11,
            page: 3,
        });
        let gaps = publish_mirror(&db, &store, &id, BASE, "raw-api-key", first).expect("first");
        assert_eq!(gaps, MirrorGaps::default());
        let mut second = MirrorSnapshot::empty();
        second.gaps.stale = vec![MirrorSection::Collections, MirrorSection::OnDeck];
        let gaps = publish_mirror(&db, &store, &id, BASE, "raw-api-key", second).expect("second");
        assert_eq!(
            gaps.stale,
            [MirrorSection::Collections, MirrorSection::OnDeck]
        );
        let collections = visible_kavita_collections(&db, &id).expect("collections");
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "Favourites");
        let shown = visible_kavita_mirror(&db, &id).expect("mirror");
        assert!(shown
            .shelves
            .get("on-deck")
            .unwrap_or(&Vec::new())
            .is_empty());
        assert!(shown.bookmarks.is_empty());
    }

    #[test]
    fn gap_notes_use_the_chip_names() {
        assert_eq!(MirrorGaps::default().note(), None);
        let one = MirrorGaps {
            stale: vec![MirrorSection::Collections],
        };
        assert_eq!(
            one.note().as_deref(),
            Some("Collections could not be read.")
        );
        let many = MirrorGaps {
            stale: vec![
                MirrorSection::OnDeck,
                MirrorSection::ReadingLists,
                MirrorSection::WantToRead,
            ],
        };
        assert_eq!(
            many.note().as_deref(),
            Some("On Deck, Lists and Want to Read could not be read.")
        );
    }

    #[test]
    fn kavita_key_is_not_the_server_id_or_the_backup() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(
            &db,
            &store,
            "Home",
            "http://192.168.1.20:5000",
            "raw-api-key",
        )
        .expect("add");
        assert_ne!(id, "Home");
        assert_eq!(store.read("kavita", &id).expect("key"), "raw-api-key");
        rename_kavita(&db, &id, "Shelf").expect("rename");
        assert_eq!(store.read("kavita", &id).expect("kept"), "raw-api-key");
        let backup = folio_storage::export_backup(&db).expect("backup");
        assert!(!backup.contains("raw-api-key"));
        assert!(backup.contains(&secret_reference(&id)));
        assert!(backup.contains(&id));
    }

    #[test]
    fn replacing_the_url_drops_the_mirror_and_a_failed_push_blocks_the_pull() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(
            &db,
            &store,
            "Home",
            "http://192.168.1.20:5000",
            "raw-api-key",
        )
        .expect("add");
        let mut snapshot = MirrorSnapshot::empty();
        snapshot.series.push(KavitaSeries {
            id: 7,
            name: "Wings".to_owned(),
            format: "epub".to_owned(),
            library_id: 1,
        });
        publish_mirror(
            &db,
            &store,
            &id,
            "http://192.168.1.20:5000",
            "raw-api-key",
            snapshot,
        )
        .expect("publish");
        record_page(
            &db,
            &id,
            PageStop {
                series_id: 7,
                volume_id: 1,
                library_id: 1,
                chapter_id: 9,
                page: 2,
                page_count: 10,
            },
        )
        .expect("progress");
        assert!(db.catalog().expect("catalog").is_empty());
        let kept = push_with(&db, &id, "http://192.168.1.20:5000", "raw-api-key", |_| {
            Err(ServerError::Transport)
        })
        .expect("push");
        assert!(!kept);
        assert!(kavita_pending(&db, &id, 9).expect("pending").is_some());
        pull_progress(&db, &id, 9, "page:1", 1).expect("pull");
        assert_eq!(
            kavita_outbox(&db, &id).expect("outbox")[0].locator,
            "page:2"
        );
        assert_eq!(page_from_locator("page:2"), Some(2));
        replace_kavita(&db, &store, &id, "http://10.0.0.8:5000", "other-key").expect("replace");
        assert!(visible_kavita_series(&db, &id).expect("series").is_empty());
        assert!(kavita_outbox(&db, &id).expect("outbox").is_empty());
        assert_eq!(store.read("kavita", &id).expect("key"), "other-key");
    }

    fn one_series(id: i64, name: &str) -> MirrorSnapshot {
        let mut snapshot = MirrorSnapshot::empty();
        snapshot.series.push(KavitaSeries {
            id,
            name: name.to_owned(),
            format: "epub".to_owned(),
            library_id: 1,
        });
        snapshot
    }

    #[test]
    fn rows_fetched_before_the_server_changed_are_not_stored() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", BASE, "raw-api-key").expect("add");
        replace_kavita(&db, &store, &id, BASE, "other-key").expect("new key");
        let stale = publish_mirror(
            &db,
            &store,
            &id,
            BASE,
            "raw-api-key",
            one_series(7, "Wings"),
        )
        .expect_err("old key");
        assert!(matches!(stale, SessionError::Changed));
        assert_eq!(
            connection_failure(&stale),
            "The server was changed while Folio read it. Try again."
        );
        replace_kavita(&db, &store, &id, "http://kavita.test:5001", "other-key").expect("new url");
        let moved = publish_mirror(&db, &store, &id, BASE, "other-key", one_series(7, "Wings"))
            .expect_err("old url");
        assert!(matches!(moved, SessionError::Changed));
        assert!(visible_kavita_series(&db, &id).expect("series").is_empty());
        let chapters = refresh_chapters_with(
            &db,
            &store,
            &id,
            BASE,
            "other-key",
            2,
            fake_kavita(Vec::new()),
        )
        .expect_err("chapters");
        assert!(matches!(chapters, SessionError::Changed));
        forget_server(&db, &store, root.path(), &id).expect("forget");
        let gone = publish_mirror(
            &db,
            &store,
            &id,
            "http://kavita.test:5001",
            "other-key",
            one_series(7, "Wings"),
        )
        .expect_err("removed");
        assert!(matches!(gone, SessionError::Changed));
    }

    #[test]
    fn a_page_turned_during_a_push_is_kept_for_the_next_one() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", "http://127.0.0.1:9", "raw-api-key").expect("add");
        let stop = |page| PageStop {
            series_id: 1,
            volume_id: 1,
            library_id: 1,
            chapter_id: 3,
            page,
            page_count: 40,
        };
        record_page(&db, &id, stop(0)).expect("first page");
        let delivered = push_with(&db, &id, "http://127.0.0.1:9", "raw-api-key", |_| {
            record_page(&db, &id, stop(5)).map_err(|_| ServerError::Transport)
        })
        .expect("push");
        assert!(delivered);
        let left = kavita_outbox(&db, &id).expect("outbox");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].locator, "page:5");
    }

    #[test]
    fn editing_a_kavita_server_renames_or_replaces_only_what_changed() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", BASE, "raw-api-key").expect("add");
        publish_mirror(
            &db,
            &store,
            &id,
            BASE,
            "raw-api-key",
            one_series(7, "Wings"),
        )
        .expect("publish");
        update_kavita(&db, &store, &id, "Attic", BASE, "").expect("rename");
        let row = servers(&db).expect("rows").remove(0);
        assert_eq!(row.display_name, "Attic");
        assert_eq!(visible_kavita_series(&db, &id).expect("kept").len(), 1);
        assert_eq!(store.read("kavita", &id).expect("key"), "raw-api-key");
        update_kavita(&db, &store, &id, "Attic", BASE, "raw-api-key").expect("same key");
        assert_eq!(visible_kavita_series(&db, &id).expect("still").len(), 1);
        update_kavita(&db, &store, &id, "Attic", "http://kavita.test:5001", "").expect("url");
        assert!(visible_kavita_series(&db, &id).expect("dropped").is_empty());
        assert_eq!(store.read("kavita", &id).expect("kept key"), "raw-api-key");
        update_kavita(
            &db,
            &store,
            &id,
            "Attic",
            "http://kavita.test:5001",
            "new-key",
        )
        .expect("key");
        assert_eq!(store.read("kavita", &id).expect("new key"), "new-key");
        assert!(matches!(
            update_kavita(&db, &store, &id, "", "http://kavita.test:5001", ""),
            Err(SessionError::Server)
        ));
    }

    #[test]
    fn a_successful_push_is_the_only_delivery() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", "http://127.0.0.1:9", "raw-api-key").expect("add");
        record_page(
            &db,
            &id,
            PageStop {
                series_id: 1,
                volume_id: 1,
                library_id: 1,
                chapter_id: 3,
                page: 0,
                page_count: 4,
            },
        )
        .expect("progress");
        let delivered =
            push_with(&db, &id, "http://127.0.0.1:9", "raw-api-key", |_| Ok(())).expect("push");
        assert!(delivered);
        assert!(kavita_outbox(&db, &id).expect("outbox").is_empty());
    }

    #[test]
    fn forgetting_a_server_removes_the_row_the_key_and_the_backup() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(
            &db,
            &store,
            "Home",
            "http://192.168.1.20:5000",
            "raw-api-key",
        )
        .expect("add");
        let mut snapshot = MirrorSnapshot::empty();
        snapshot.series.push(KavitaSeries {
            id: 7,
            name: "Wings".to_owned(),
            format: "epub".to_owned(),
            library_id: 1,
        });
        publish_mirror(
            &db,
            &store,
            &id,
            "http://192.168.1.20:5000",
            "raw-api-key",
            snapshot,
        )
        .expect("publish");
        let directory = chapter_directory(&download_root(root.path()), &id, 9);
        std::fs::create_dir_all(&directory).expect("dir");
        std::fs::write(directory.join("page"), b"bytes").expect("page");
        let other = download_root(root.path()).join("kavita-other-1");
        std::fs::create_dir_all(&other).expect("other");
        upsert_server(
            &db,
            StoredServer {
                id: "Komga".to_owned(),
                kind: "komga".to_owned(),
                url: "http://192.168.1.20:25600".to_owned(),
                display_name: "Komga".to_owned(),
                secret_ref: Some("keyring:komga:home".to_owned()),
            },
        )
        .expect("komga");
        forget_server(&db, &store, root.path(), &id).expect("forget");
        assert!(servers(&db).expect("rows").iter().all(|row| row.id != id));
        assert!(matches!(
            store.read("kavita", &id),
            Err(folio_platform::SecretError::Missing)
        ));
        let backup = export_backup(&db).expect("backup");
        assert!(!backup.contains("raw-api-key"));
        assert!(!backup.contains(&secret_reference(&id)));
        assert!(visible_kavita_series(&db, &id).expect("series").is_empty());
        assert!(!directory.exists());
        assert!(other.exists());
        assert!(matches!(
            forget_server(&db, &store, root.path(), "missing"),
            Err(SessionError::Server)
        ));
        forget_server(&db, &store, root.path(), "Komga").expect("komga");
        assert!(servers(&db).expect("rows").is_empty());
    }

    #[test]
    fn a_single_raw_opds_secret_becomes_a_kavita_reference() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        upsert_server(
            &db,
            StoredServer {
                id: "Home".to_owned(),
                kind: "opds".to_owned(),
                url: "http://192.168.1.20:5000".to_owned(),
                display_name: "Home".to_owned(),
                secret_ref: Some("raw-api-key".to_owned()),
            },
        )
        .expect("seed");
        let id = rehome_raw_opds(&db, &store)
            .expect("rehome")
            .expect("moved");
        assert_ne!(id, "Home");
        let rows = servers(&db).expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "kavita");
        assert_eq!(
            rows[0].secret_ref.as_deref(),
            Some(secret_reference(&id).as_str())
        );
        assert_eq!(store.read("kavita", &id).expect("key"), "raw-api-key");
        let backup = export_backup(&db).expect("backup");
        assert!(!backup.contains("raw-api-key"));
        assert!(backup.contains(&secret_reference(&id)));
    }

    #[test]
    fn two_raw_opds_rows_stay_put() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        for (id, secret) in [("Home", "raw-api-key"), ("Other", "other-api-key")] {
            upsert_server(
                &db,
                StoredServer {
                    id: id.to_owned(),
                    kind: "opds".to_owned(),
                    url: "http://192.168.1.20:5000".to_owned(),
                    display_name: id.to_owned(),
                    secret_ref: Some(secret.to_owned()),
                },
            )
            .expect("seed");
        }
        assert!(rehome_raw_opds(&db, &store).expect("rehome").is_none());
        let rows = servers(&db).expect("rows");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.kind == "opds"));
    }

    #[test]
    fn a_keyring_row_and_a_raw_komga_row_are_not_adopted() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        upsert_server(
            &db,
            StoredServer {
                id: "Home".to_owned(),
                kind: "opds".to_owned(),
                url: "http://192.168.1.20:5000".to_owned(),
                display_name: "Home".to_owned(),
                secret_ref: Some("keyring:opds:home".to_owned()),
            },
        )
        .expect("opds");
        upsert_server(
            &db,
            StoredServer {
                id: "Komga".to_owned(),
                kind: "komga".to_owned(),
                url: "http://192.168.1.20:25600".to_owned(),
                display_name: "Komga".to_owned(),
                secret_ref: Some("raw-api-key".to_owned()),
            },
        )
        .expect("komga");
        assert!(rehome_raw_opds(&db, &store).expect("rehome").is_none());
        assert_eq!(servers(&db).expect("rows").len(), 2);
    }

    #[test]
    fn probe_of_a_closed_port_hides_the_key() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let mut base = url::Url::parse("http://127.0.0.1/").expect("url");
        base.set_port(Some(port)).expect("port");
        let error = probe_kavita(base.as_str(), "raw-api-key").expect_err("closed");
        let message = connection_failure(&error);
        assert_eq!(message, "The server could not be reached.");
        assert!(!message.contains("raw-api-key"));
        assert!(!message.contains("127.0.0.1"));
    }

    #[test]
    fn refresh_status_never_says_sent() {
        let offline = "Komga is offline. Progress stays on this device.";
        let gap = Some("Collections could not be read.");
        let cases = [
            refresh_status(false, false, None, false, None, offline),
            refresh_status(true, false, None, false, None, offline),
            refresh_status(true, true, None, false, None, offline),
            refresh_status(true, true, None, true, None, offline),
            refresh_status(
                true,
                false,
                Some("The server refused the Auth Key."),
                false,
                gap,
                offline,
            ),
            refresh_status(
                true,
                true,
                Some("http://secret.example/key"),
                false,
                None,
                offline,
            ),
            refresh_status(true, false, None, false, gap, offline),
            refresh_status(true, true, None, true, gap, offline),
        ];
        assert_eq!(cases[0], offline);
        assert_eq!(cases[1], "Kavita was refreshed.");
        assert_eq!(cases[2], "Progress was delivered. Kavita was refreshed.");
        assert_eq!(
            cases[3],
            "Some progress was delivered. The rest stays on this device."
        );
        assert_eq!(cases[4], "The server refused the Auth Key.");
        assert_eq!(cases[5], "Kavita could not be contacted.");
        assert_eq!(
            cases[6],
            "Kavita was refreshed. Collections could not be read."
        );
        assert_eq!(
            cases[7],
            "Some progress was delivered. The rest stays on this device. \
             Collections could not be read."
        );
        assert!(cases.iter().all(|message| !message.contains("sent")));
    }

    #[test]
    fn connection_failure_names_a_refusal() {
        let message = connection_failure(&SessionError::Remote(ServerError::Refused));
        assert_eq!(message, "The server refused the Auth Key.");
        assert_eq!(
            connection_failure(&SessionError::Remote(ServerError::Feed)),
            "Kavita answered, but the reply could not be read."
        );
        let missing = connection_failure(&SessionError::Remote(ServerError::Dns));
        assert_eq!(missing, "The server name could not be found.");
        assert!(!missing.contains("://"));
        assert_eq!(
            connection_failure(&SessionError::Hidden),
            "That series is not in the library."
        );
    }

    fn complete_chapter(root: &std::path::Path, server_id: &str, chapter_id: i64) {
        let directory = chapter_directory(&download_root(root), server_id, chapter_id);
        std::fs::create_dir_all(&directory).expect("dir");
        std::fs::write(directory.join("page-0.html"), b"<p>hi</p>").expect("page");
        std::fs::write(directory.join("pages.txt"), "0 0 0\n").expect("pages");
        std::fs::write(directory.join("complete"), b"").expect("complete");
    }

    #[test]
    fn volumes_refused_hides_the_series_and_its_download() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", BASE, "raw-api-key").expect("add");
        publish_mirror(&db, &store, &id, BASE, "raw-api-key", {
            let mut snapshot = MirrorSnapshot::empty();
            snapshot.series.push(KavitaSeries {
                id: 1,
                name: "Wings".to_owned(),
                format: "epub".to_owned(),
                library_id: 1,
            });
            snapshot.series.push(KavitaSeries {
                id: 2,
                name: "Tide".to_owned(),
                format: "epub".to_owned(),
                library_id: 1,
            });
            snapshot
        })
        .expect("publish");
        refresh_chapters_with(
            &db,
            &store,
            &id,
            BASE,
            "raw-api-key",
            1,
            fake_kavita(Vec::new()),
        )
        .expect("chapters");
        enqueue_kavita_download(
            &db,
            &format!("kavita-{id}-11"),
            &id,
            "http://kavita.test:5000/chapter",
            1,
        )
        .expect("queue");
        complete_chapter(root.path(), &id, 11);
        let mutated = folio_storage::kavita_mirror_mutated(&db, &id).expect("mutated");
        let error = admit_kavita_series_with(
            &db,
            &store,
            root.path(),
            &id,
            (BASE, "raw-api-key"),
            1,
            fake_kavita(vec![("/api/Series/volumes", Err(ServerError::Refused))]),
        )
        .expect_err("hidden");
        assert!(matches!(error, SessionError::Hidden));
        assert_eq!(
            connection_failure(&error),
            "That series is not in the library."
        );
        let names: Vec<String> = visible_kavita_series(&db, &id)
            .expect("series")
            .into_iter()
            .map(|row| row.name)
            .collect();
        assert_eq!(names, ["Tide"]);
        assert!(!chapter_directory(&download_root(root.path()), &id, 11).exists());
        assert_eq!(
            folio_storage::kavita_mirror_mutated(&db, &id).expect("mutated"),
            mutated + 1
        );
    }

    #[test]
    fn volumes_transport_opens_a_listed_complete_download() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", BASE, "raw-api-key").expect("add");
        publish_mirror(
            &db,
            &store,
            &id,
            BASE,
            "raw-api-key",
            one_series(1, "Wings"),
        )
        .expect("publish");
        refresh_chapters_with(
            &db,
            &store,
            &id,
            BASE,
            "raw-api-key",
            1,
            fake_kavita(Vec::new()),
        )
        .expect("chapters");
        complete_chapter(root.path(), &id, 11);
        let error = admit_kavita_series_with(
            &db,
            &store,
            root.path(),
            &id,
            (BASE, "raw-api-key"),
            1,
            fake_kavita(vec![("/api/Series/volumes", Err(ServerError::Transport))]),
        )
        .expect_err("transport");
        assert!(matches!(
            error,
            SessionError::Mirror {
                step: MirrorStep::Chapters,
                error: ServerError::Transport,
            }
        ));
        assert!(chapter_can_open(
            directory_complete(&chapter_directory(&download_root(root.path()), &id, 11), 1),
            false
        ));
        assert_eq!(visible_kavita_series(&db, &id).expect("kept").len(), 1);
        assert!(chapter_directory(&download_root(root.path()), &id, 11).exists());
    }

    #[test]
    fn empty_volumes_do_not_hide_or_delete_a_download() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", BASE, "raw-api-key").expect("add");
        publish_mirror(
            &db,
            &store,
            &id,
            BASE,
            "raw-api-key",
            one_series(1, "Wings"),
        )
        .expect("publish");
        refresh_chapters_with(
            &db,
            &store,
            &id,
            BASE,
            "raw-api-key",
            1,
            fake_kavita(Vec::new()),
        )
        .expect("chapters");
        enqueue_kavita_download(
            &db,
            &format!("kavita-{id}-11"),
            &id,
            "http://kavita.test:5000/chapter",
            1,
        )
        .expect("queue");
        complete_chapter(root.path(), &id, 11);
        admit_kavita_series_with(
            &db,
            &store,
            root.path(),
            &id,
            (BASE, "raw-api-key"),
            1,
            fake_kavita(vec![("/api/Series/volumes", Ok("[]"))]),
        )
        .expect("empty");
        assert!(visible_kavita_chapters(&db, &id, 1)
            .expect("chapters")
            .is_empty());
        assert_eq!(visible_kavita_series(&db, &id).expect("series").len(), 1);
        assert!(chapter_directory(&download_root(root.path()), &id, 11).exists());
    }

    #[test]
    fn a_stale_refresh_does_not_restore_a_hidden_series() {
        let root = tempfile::tempdir().expect("temp");
        let db = open_database(root.path()).expect("db");
        let store = SecretStore::open(root.path()).expect("store");
        let id = add_kavita(&db, &store, "Home", BASE, "raw-api-key").expect("add");
        let snapshot = mirror_with(BASE, "raw-api-key", fake_kavita(Vec::new())).expect("fetch");
        let mutated = folio_storage::kavita_mirror_mutated(&db, &id).expect("mutated");
        publish_mirror(&db, &store, &id, BASE, "raw-api-key", snapshot.clone()).expect("first");
        admit_kavita_series_with(
            &db,
            &store,
            root.path(),
            &id,
            (BASE, "raw-api-key"),
            1,
            fake_kavita(vec![("/api/Series/volumes", Err(ServerError::Refused))]),
        )
        .expect_err("hidden");
        let stale = folio_storage::publish_kavita_mirror(
            &db,
            &id,
            super::mirror_rows(snapshot),
            mutated,
            super::same_account(&store, BASE, "raw-api-key"),
        )
        .expect("publish");
        assert!(!stale);
        let names: Vec<i64> = visible_kavita_series(&db, &id)
            .expect("series")
            .into_iter()
            .map(|row| row.series_id)
            .collect();
        assert!(!names.contains(&1));
    }

    #[test]
    fn covers_to_fetch_keeps_screen_order_skips_cached_and_caps() {
        let mut cached = std::collections::BTreeMap::new();
        cached.insert(2, "data:image/jpeg;base64,AA==".to_owned());
        cached.insert(5, String::new());
        let shown = [4, 2, 4, 7, 5, 9, 1];
        assert_eq!(
            super::covers_to_fetch(&shown, &cached, 10),
            vec![4, 7, 9, 1]
        );
        // The cap counts cards on screen (4 and 2), not misses, so only 4 is left to fetch.
        assert_eq!(super::covers_to_fetch(&shown, &cached, 2), vec![4]);
        assert!(super::covers_to_fetch(&[], &cached, 10).is_empty());
        // Once the first `limit` cards are cached, nothing is left even if later cards are not.
        cached.insert(4, String::new());
        assert!(super::covers_to_fetch(&shown, &cached, 2).is_empty());
    }

    #[test]
    fn list_page_clamps_the_index_and_slices_the_list() {
        let first = super::list_page(13782, 0, 40);
        assert_eq!(
            (first.index, first.pages, first.start, first.end),
            (0, 345, 0, 40)
        );
        let past = super::list_page(13782, 999, 40);
        assert_eq!((past.index, past.start, past.end), (344, 13760, 13782));
        let empty = super::list_page(0, 3, 40);
        assert_eq!(
            (empty.index, empty.pages, empty.start, empty.end),
            (0, 1, 0, 0)
        );
        let exact = super::list_page(80, 1, 40);
        assert_eq!(
            (exact.index, exact.pages, exact.start, exact.end),
            (1, 2, 40, 80)
        );
    }
}
