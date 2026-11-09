//! Two foreground passes. Pass 1 lists. Pass 2 opens only rows that need it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use folio_core::{FileType, SliceSource};
use folio_formats::{
    comic_info_series, open_cb7, open_cbr, open_cbt, open_cbz, open_epub, open_image_folder,
    open_mobi, pdf_page_count, render_pdf_png, ImageEntry,
};
use folio_platform::{list_directory, ListedPath};
use folio_storage::{CatalogItem, Database, StorageError};
use thiserror::Error;

/// How far a scan runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanDepth {
    /// Identity rows only. No archive or PDF is opened.
    Identity,
    /// Identity, then enrichment for rows whose fingerprint is unfinished.
    Enrich,
}

/// Inputs for one scan over an already built listing.
#[derive(Debug)]
pub struct ScanRequest<'a> {
    /// Directory or cursor entries. Bytes of books are not included.
    pub entries: &'a [ListedPath],
    /// When false, missing rows are kept. A loading cursor is not complete.
    pub complete: bool,
    /// Directory for cover files. Presence is the cover cache.
    pub cover_dir: &'a Path,
    /// Set when the library screen is left. Checked between rows.
    pub cancel: &'a AtomicBool,
    /// Whether pass 2 runs.
    pub depth: ScanDepth,
}

/// A row after pass 1, before pass 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRow {
    /// Content source, relative to the library root.
    pub source: String,
    /// Listing size. Unknown stays `None`.
    pub size: Option<u64>,
    /// Listing modification time in Unix milliseconds.
    pub modified_ms: Option<i64>,
    /// Format from the extension, or from a bounded sniff when that is not enough.
    pub format: FileType,
    /// Filename or folder label.
    pub label: String,
    /// Parent folder, or a filename guess when the book sits at the root.
    pub series_guess: String,
    /// Empty until pass 2. Image folders do not fill this from the child count.
    pub page_count: Option<u32>,
    /// Page images directly inside an image folder.
    pub image_child_count: Option<u32>,
}

/// Fields pass 2 copies out of a parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPublication {
    /// Pages, spine items, or image-folder images.
    pub page_count: u32,
    /// OPF title when the book has one.
    pub title: Option<String>,
    /// ComicInfo series when the book or a sidecar has one.
    pub series: Option<String>,
    /// Cover bytes. Empty means the parser found none.
    pub cover: Option<Vec<u8>>,
}

/// Rows stored by a scan, and which sources pass 2 opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOutcome {
    /// Catalog after the writes from this call.
    pub rows: Vec<CatalogItem>,
    /// Sources whose bytes were opened, in open order.
    pub opened: Vec<String>,
    /// How many times pass 1 sniffed a header. Known extensions do not sniff.
    pub sniffs: u32,
}

/// Why a scan stopped.
#[derive(Debug, Error)]
pub enum ScanError {
    /// The catalog database rejected a write or a read.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The listing could not be read.
    #[error(transparent)]
    Listing(#[from] folio_platform::PlatformError),
    /// A publication parser failed.
    #[error(transparent)]
    Format(#[from] folio_formats::FormatError),
    /// A cover or a book file could not be read.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A relative path left the library root.
    #[error("a catalog path escapes the library root")]
    Escape,
    /// A page count did not fit in the catalog column.
    #[error("a catalog number did not fit")]
    Number,
}

struct Fact {
    relative: String,
    parent: String,
    name: String,
    size: Option<u64>,
    modified_ms: Option<i64>,
    mime: Option<String>,
}

struct Part {
    name: String,
    size: Option<u64>,
    modified_ms: Option<i64>,
}

struct Pub {
    id: String,
    format: FileType,
    size: Option<u64>,
    modified_ms: Option<i64>,
    label: String,
    series_guess: String,
    image_child_count: Option<u32>,
    fingerprint: String,
    facts_known: bool,
}

/// Pass 1 over `entries`. The sniff callback is the only file read, and only for
/// an unknown extension or an extension that disagrees with the provider MIME.
pub fn identify(
    entries: &[ListedPath],
    sniff: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Vec<IdentityRow> {
    publications(entries, sniff)
        .into_iter()
        .map(|publication| IdentityRow {
            source: publication.id,
            size: publication.size,
            modified_ms: publication.modified_ms,
            format: publication.format,
            label: publication.label,
            series_guess: publication.series_guess,
            page_count: None,
            image_child_count: publication.image_child_count,
        })
        .collect()
}

/// List `root` and scan it. Listing uses directory metadata, not file bytes.
///
/// # Errors
///
/// Returns an error when the walk or a catalog write fails. One unreadable
/// book does not fail the scan; that row stays unfinished.
pub fn scan_directory(
    root: &Path,
    db: &Database,
    cover_dir: &Path,
    cancel: &AtomicBool,
    depth: ScanDepth,
) -> Result<ScanOutcome, ScanError> {
    let entries = list_directory(root)?;
    let request = ScanRequest {
        entries: &entries,
        complete: true,
        cover_dir,
        cancel,
        depth,
    };
    scan_listing(
        db,
        &request,
        &mut |relative| read_prefix(root, relative),
        &mut |relative, kind| open_listed(root, relative, kind),
    )
}

/// Scan a listing the caller already has. `open` is pass 2.
///
/// # Errors
///
/// Returns an error when a catalog write fails or a cover cannot be stored.
pub fn scan_listing(
    db: &Database,
    request: &ScanRequest<'_>,
    sniff: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    open: &mut dyn FnMut(&str, FileType) -> Result<ParsedPublication, ScanError>,
) -> Result<ScanOutcome, ScanError> {
    if request.cancel.load(Ordering::SeqCst) {
        return Ok(ScanOutcome {
            rows: db.catalog()?,
            opened: Vec::new(),
            sniffs: 0,
        });
    }
    let mut sniffs = 0u32;
    let mut counting = |relative: &str| {
        sniffs = sniffs.saturating_add(1);
        sniff(relative)
    };
    let pubs = publications(request.entries, &mut counting);
    let previous = db
        .catalog()?
        .into_iter()
        .map(|item| {
            // clone: the map key is owned separately from the stored row
            (item.id.clone(), item)
        })
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    for publication in &pubs {
        if request.cancel.load(Ordering::SeqCst) {
            return Ok(ScanOutcome {
                rows: db.catalog()?,
                opened: Vec::new(),
                sniffs,
            });
        }
        db.upsert_item(identity_item(publication, previous.get(&publication.id)))?;
        seen.insert(publication.id.clone());
    }
    if request.complete && !request.cancel.load(Ordering::SeqCst) {
        for item in db.catalog()? {
            if !seen.contains(&item.id) {
                db.delete_item(&item.id)?;
            }
        }
    }
    if request.depth == ScanDepth::Identity || request.cancel.load(Ordering::SeqCst) {
        return Ok(ScanOutcome {
            rows: db.catalog()?,
            opened: Vec::new(),
            sniffs,
        });
    }
    let mut opened = Vec::new();
    for publication in &pubs {
        if request.cancel.load(Ordering::SeqCst) {
            break;
        }
        let Some(mut item) = db
            .catalog()?
            .into_iter()
            .find(|row| row.id == publication.id)
        else {
            continue;
        };
        if !needs_open(&item, publication, request.cover_dir) {
            continue;
        }
        opened.push(publication.id.clone());
        match open(&publication.id, publication.format) {
            Ok(parsed) => {
                let cover = parsed.cover.as_deref().unwrap_or_default();
                write_cover(request.cover_dir, &publication.id, cover)?;
                apply_enrichment(&mut item, publication, &parsed)?;
                db.upsert_item(item)?;
            }
            Err(error) => {
                tracing::debug!(%error, id = %publication.id, "enrichment did not finish");
            }
        }
    }
    Ok(ScanOutcome {
        rows: db.catalog()?,
        opened,
        sniffs,
    })
}

/// Open one publication under `root`. Callers use this from pass 2 only.
///
/// # Errors
///
/// Returns an error when the file cannot be read, escapes `root`, or is not a
/// supported book.
pub fn open_listed(
    root: &Path,
    relative: &str,
    kind: FileType,
) -> Result<ParsedPublication, ScanError> {
    match kind {
        FileType::ImageFolder => open_folder(root, relative),
        FileType::Epub => open_epub_at(root, relative),
        FileType::Cbz | FileType::Cb7 | FileType::Cbt | FileType::Cbr => {
            open_comic(root, relative, kind)
        }
        FileType::Pdf => open_pdf_at(root, relative),
        FileType::Mobi => open_mobi_at(root, relative),
        FileType::Unknown => Err(ScanError::Format(folio_formats::FormatError::Invalid {
            kind: "catalog",
        })),
    }
}

/// Cover file for `id` inside `dir`.
pub fn cover_cache_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{}.cover", hex(id.as_bytes())))
}

fn publications(
    entries: &[ListedPath],
    sniff: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Vec<Pub> {
    let mut files = Vec::new();
    let mut directories = BTreeMap::new();
    let mut children: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for entry in entries {
        let parent = parent_of(&entry.relative).to_owned();
        let name = name_of(&entry.relative).to_owned();
        let fact = Fact {
            relative: entry.relative.clone(),
            parent,
            name,
            size: entry.size,
            modified_ms: entry.modified_ms,
            mime: entry.mime.clone(),
        };
        if entry.is_dir {
            directories.insert(fact.relative.clone(), fact);
            continue;
        }
        let index = files.len();
        children.entry(fact.parent.clone()).or_default().push(index);
        files.push(fact);
    }
    let image_parents = children
        .iter()
        .filter(|(_, indexes)| folder_is_images(&files, indexes))
        .map(|(parent, _)| parent.clone())
        .collect::<BTreeSet<_>>();
    let mut pubs = Vec::new();
    for parent in &image_parents {
        if let Some(publication) = image_publication(parent, &files, &children, &directories) {
            pubs.push(publication);
        }
    }
    for fact in &files {
        if image_parents.contains(&fact.parent) || is_sidecar(&fact.name) {
            continue;
        }
        let Some(format) = resolve_kind(&fact.name, fact.mime.as_deref(), &fact.relative, sniff)
        else {
            continue;
        };
        let sidecars = sidecar_parts(&files, &fact.parent);
        pubs.push(book_publication(fact, format, sidecars));
    }
    pubs.sort_by(|left, right| left.id.cmp(&right.id));
    pubs
}

fn image_publication(
    parent: &str,
    files: &[Fact],
    children: &BTreeMap<String, Vec<usize>>,
    directories: &BTreeMap<String, Fact>,
) -> Option<Pub> {
    let indexes = children.get(parent)?;
    let mut parts = Vec::new();
    let mut images = 0u32;
    for index in indexes {
        let fact = files.get(*index)?;
        if is_sidecar(&fact.name) || is_page(&fact.name) {
            if is_page(&fact.name) && !is_sidecar(&fact.name) {
                images = images.saturating_add(1);
            }
            parts.push(Part {
                name: fact.name.clone(),
                size: fact.size,
                modified_ms: fact.modified_ms,
            });
        }
    }
    if images == 0 {
        return None;
    }
    let directory = directories.get(parent);
    let (fingerprint, facts_known) = fingerprint(
        directory.and_then(|fact| fact.size),
        directory.and_then(|fact| fact.modified_ms),
        &parts,
    );
    let id = if parent.is_empty() {
        return None;
    } else {
        parent.to_owned()
    };
    Some(Pub {
        label: crate::filename_label(name_of(&id)).to_owned(),
        series_guess: series_guess(&id, true),
        id,
        format: FileType::ImageFolder,
        size: directory.and_then(|fact| fact.size),
        modified_ms: directory.and_then(|fact| fact.modified_ms),
        image_child_count: Some(images),
        fingerprint,
        facts_known,
    })
}

fn book_publication(fact: &Fact, format: FileType, sidecars: Vec<Part>) -> Pub {
    let (fingerprint, facts_known) = fingerprint(fact.size, fact.modified_ms, &sidecars);
    Pub {
        label: crate::filename_label(&fact.name).to_owned(),
        series_guess: series_guess(&fact.relative, false),
        id: fact.relative.clone(),
        format,
        size: fact.size,
        modified_ms: fact.modified_ms,
        image_child_count: None,
        fingerprint,
        facts_known,
    }
}

fn folder_is_images(files: &[Fact], indexes: &[usize]) -> bool {
    let mut images = 0;
    let mut books = 0;
    for index in indexes {
        let Some(fact) = files.get(*index) else {
            continue;
        };
        if is_sidecar(&fact.name) {
            continue;
        }
        if is_page(&fact.name) {
            images += 1;
        } else if extension_book(&fact.name).is_some() {
            books += 1;
        }
    }
    images > 0 && books == 0
}

fn sidecar_parts(files: &[Fact], parent: &str) -> Vec<Part> {
    files
        .iter()
        .filter(|fact| fact.parent == parent && is_sidecar(&fact.name))
        .map(|fact| Part {
            name: fact.name.clone(),
            size: fact.size,
            modified_ms: fact.modified_ms,
        })
        .collect()
}

fn fingerprint(size: Option<u64>, modified_ms: Option<i64>, parts: &[Part]) -> (String, bool) {
    let mut known = size.is_some() && modified_ms.is_some();
    let mut ordered = parts
        .iter()
        .map(|part| Part {
            name: part.name.clone(),
            size: part.size,
            modified_ms: part.modified_ms,
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.name.cmp(&right.name));
    let mut text = format!("size={}\nmtime={}", token_u64(size), token_i64(modified_ms));
    for part in ordered {
        if part.size.is_none() || part.modified_ms.is_none() {
            known = false;
        }
        text.push('\n');
        text.push_str(&part.name);
        text.push(' ');
        text.push_str(&token_u64(part.size));
        text.push(' ');
        text.push_str(&token_i64(part.modified_ms));
    }
    (text, known)
}

fn identity_item(publication: &Pub, previous: Option<&CatalogItem>) -> CatalogItem {
    let same = previous.is_some_and(|row| row.fingerprint == publication.fingerprint);
    let previous = same.then_some(previous).flatten();
    CatalogItem {
        // clone: the writer thread stores the row while pass 2 still borrows the publication
        id: publication.id.clone(),
        source: publication.id.clone(),
        label: publication.label.clone(),
        file_type: type_name(publication.format).to_owned(),
        size: publication.size.and_then(|size| i64::try_from(size).ok()),
        modified_ms: publication.modified_ms,
        series_guess: publication.series_guess.clone(),
        series: previous.and_then(|row| row.series.clone()),
        title: previous
            .map(|row| row.title.clone())
            .unwrap_or_else(|| publication.label.clone()),
        page_count: previous.and_then(|row| row.page_count),
        image_child_count: publication.image_child_count.map(i64::from),
        fingerprint: publication.fingerprint.clone(),
        enriched_fingerprint: previous.and_then(|row| row.enriched_fingerprint.clone()),
    }
}

fn needs_open(item: &CatalogItem, publication: &Pub, cover_dir: &Path) -> bool {
    if !publication.facts_known {
        return true;
    }
    if item.enriched_fingerprint.as_deref() != Some(publication.fingerprint.as_str()) {
        return true;
    }
    !cover_cache_path(cover_dir, &publication.id).is_file()
}

fn apply_enrichment(
    item: &mut CatalogItem,
    publication: &Pub,
    parsed: &ParsedPublication,
) -> Result<(), ScanError> {
    item.page_count = Some(i64::from(parsed.page_count));
    if let Some(series) = &parsed.series {
        item.series = Some(series.clone());
    }
    if let Some(title) = &parsed.title {
        item.title = title.clone();
    }
    item.enriched_fingerprint = publication
        .facts_known
        .then(|| publication.fingerprint.clone());
    Ok(())
}

fn open_epub_at(root: &Path, relative: &str) -> Result<ParsedPublication, ScanError> {
    let bytes = fs::read(contained(root, relative)?)?;
    let mut source = SliceSource::new(&bytes);
    let book = open_epub(&mut source)?;
    let title = if book.title.is_empty() {
        None
    } else {
        Some(book.title)
    };
    Ok(ParsedPublication {
        page_count: u32::try_from(book.chapters.len()).map_err(|_| ScanError::Number)?,
        title,
        series: None,
        cover: book.cover,
    })
}

fn open_comic(root: &Path, relative: &str, kind: FileType) -> Result<ParsedPublication, ScanError> {
    let bytes = fs::read(contained(root, relative)?)?;
    let sidecar_cover =
        read_sibling(root, relative, "cover.jpg")?.or(read_sibling(root, relative, "cover.png")?);
    let mut source = SliceSource::new(&bytes);
    let comic = match kind {
        FileType::Cbz => open_cbz(&mut source, sidecar_cover.as_deref())?,
        FileType::Cb7 => open_cb7(&mut source, sidecar_cover.as_deref())?,
        FileType::Cbt => open_cbt(&mut source, sidecar_cover.as_deref())?,
        FileType::Cbr => open_cbr(&mut source, sidecar_cover.as_deref())?,
        _ => {
            return Err(ScanError::Format(folio_formats::FormatError::Invalid {
                kind: "catalog",
            }))
        }
    };
    let sidecar_series = read_sibling(root, relative, "ComicInfo.xml")?
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| comic_info_series(&text));
    Ok(ParsedPublication {
        page_count: u32::try_from(comic.pages.len()).map_err(|_| ScanError::Number)?,
        title: None,
        series: sidecar_series.or(comic.series),
        cover: comic.cover,
    })
}

fn open_pdf_at(root: &Path, relative: &str) -> Result<ParsedPublication, ScanError> {
    let bytes = fs::read(contained(root, relative)?)?;
    let page_count = pdf_page_count(&bytes)?;
    let cover = match render_pdf_png(&bytes, 0) {
        Ok(png) => Some(png),
        Err(error) => {
            tracing::debug!(%error, "pdf cover was not drawn");
            None
        }
    };
    Ok(ParsedPublication {
        page_count,
        title: None,
        series: None,
        cover,
    })
}

fn open_mobi_at(root: &Path, relative: &str) -> Result<ParsedPublication, ScanError> {
    let bytes = fs::read(contained(root, relative)?)?;
    let mut source = SliceSource::new(&bytes);
    let book = open_mobi(&mut source)?;
    let title = if book.title.is_empty() {
        None
    } else {
        Some(book.title)
    };
    Ok(ParsedPublication {
        page_count: 1,
        title,
        series: None,
        cover: None,
    })
}

fn open_folder(root: &Path, relative: &str) -> Result<ParsedPublication, ScanError> {
    let dir = contained(root, relative)?;
    let mut names = Vec::new();
    let mut bytes = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_sidecar(name) || !is_page(name) {
            continue;
        }
        let child = format!("{relative}/{name}");
        let path = contained(root, &child)?;
        names.push(name.to_owned());
        bytes.push(fs::read(path)?);
    }
    let entries = names
        .iter()
        .zip(bytes.iter())
        .map(|(name, data)| ImageEntry {
            name: name.as_str(),
            bytes: data.as_slice(),
        })
        .collect::<Vec<_>>();
    let cover = read_child(&dir, "cover.jpg")?.or(read_child(&dir, "cover.png")?);
    let comic = open_image_folder(&entries, cover.as_deref())?;
    let series = read_child(&dir, "ComicInfo.xml")?
        .and_then(|data| String::from_utf8(data).ok())
        .and_then(|text| comic_info_series(&text));
    Ok(ParsedPublication {
        page_count: u32::try_from(comic.pages.len()).map_err(|_| ScanError::Number)?,
        title: None,
        series,
        cover: comic.cover,
    })
}

fn read_child(dir: &Path, name: &str) -> Result<Option<Vec<u8>>, ScanError> {
    let path = dir.join(name);
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(fs::read(path)?))
}

fn read_sibling(root: &Path, relative: &str, name: &str) -> Result<Option<Vec<u8>>, ScanError> {
    let parent = parent_of(relative);
    let joined = if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    };
    let path = rooted(root, &joined)?;
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(fs::read(contained(root, &joined)?)?))
}

fn write_cover(dir: &Path, id: &str, bytes: &[u8]) -> Result<(), ScanError> {
    fs::create_dir_all(dir)?;
    let path = cover_cache_path(dir, id);
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, &path)?;
    Ok(())
}

fn read_prefix(root: &Path, relative: &str) -> Option<Vec<u8>> {
    let path = rooted(root, relative).ok()?;
    let mut file = File::open(path).ok()?;
    let mut buf = [0u8; 262];
    let read = file.read(&mut buf).ok()?;
    Some(buf[..read].to_vec())
}

fn contained(root: &Path, relative: &str) -> Result<PathBuf, ScanError> {
    let path = rooted(root, relative)?;
    let root = root.canonicalize()?;
    let canon = path.canonicalize()?;
    if canon.starts_with(&root) {
        Ok(canon)
    } else {
        Err(ScanError::Escape)
    }
}

fn rooted(root: &Path, relative: &str) -> Result<PathBuf, ScanError> {
    if relative.is_empty() || relative.starts_with('/') || relative.contains('\0') {
        return Err(ScanError::Escape);
    }
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(ScanError::Escape);
        }
        path.push(part);
    }
    Ok(path)
}

fn resolve_kind(
    name: &str,
    mime: Option<&str>,
    relative: &str,
    sniff: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Option<FileType> {
    let by_ext = extension_book(name);
    let by_mime = mime.and_then(mime_kind);
    match (by_ext, by_mime) {
        (Some(extension), Some(from_mime)) if extension != from_mime => {
            sniff_kind(relative, sniff).or(Some(extension))
        }
        (Some(extension), _) => Some(extension),
        (None, _) => sniff_kind(relative, sniff),
    }
}

fn sniff_kind(relative: &str, sniff: &mut dyn FnMut(&str) -> Option<Vec<u8>>) -> Option<FileType> {
    let bytes = sniff(relative)?;
    let end = bytes.len().min(262);
    match FileType::detect("sniff.bin", &bytes[..end]) {
        FileType::Unknown | FileType::ImageFolder => None,
        kind => Some(kind),
    }
}

fn extension_book(name: &str) -> Option<FileType> {
    match FileType::detect(name, b"") {
        FileType::Unknown | FileType::ImageFolder => None,
        kind => Some(kind),
    }
}

fn mime_kind(mime: &str) -> Option<FileType> {
    let mime = mime.split(';').next()?.trim().to_ascii_lowercase();
    match mime.as_str() {
        "application/pdf" => Some(FileType::Pdf),
        "application/epub+zip" => Some(FileType::Epub),
        "application/vnd.comicbook+zip" | "application/x-cbz" => Some(FileType::Cbz),
        "application/x-cb7" | "application/x-7z-compressed" => Some(FileType::Cb7),
        "application/x-cbt" | "application/x-tar" => Some(FileType::Cbt),
        "application/vnd.comicbook-rar"
        | "application/x-rar-compressed"
        | "application/vnd.rar" => Some(FileType::Cbr),
        "application/x-mobipocket-ebook" | "application/vnd.amazon.ebook" => Some(FileType::Mobi),
        _ => None,
    }
}

fn series_guess(relative: &str, folder: bool) -> String {
    let parent = parent_of(relative);
    if folder {
        if parent.is_empty() {
            return name_of(relative).to_owned();
        }
        return name_of(parent).to_owned();
    }
    if parent.is_empty() {
        return filename_series(crate::filename_label(name_of(relative)));
    }
    name_of(parent).to_owned()
}

fn filename_series(stem: &str) -> String {
    let mut text = stem.to_owned();
    loop {
        let next = strip_tail(text.trim_end());
        if next == text.trim_end() {
            break;
        }
        text = next.to_owned();
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        stem.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn strip_tail(text: &str) -> &str {
    if let Some(open) = text.rfind(" (") {
        if text.ends_with(')') {
            let inside = &text[open.saturating_add(2)..text.len().saturating_sub(1)];
            if inside.len() == 4 && inside.bytes().all(|byte| byte.is_ascii_digit()) {
                return &text[..open];
            }
        }
    }
    let Some((head, tail)) = text.rsplit_once(' ') else {
        return text;
    };
    let digits = tail
        .strip_prefix('v')
        .or_else(|| tail.strip_prefix('V'))
        .or_else(|| tail.strip_prefix('#'))
        .unwrap_or(tail);
    if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        head
    } else {
        text
    }
}

fn parent_of(relative: &str) -> &str {
    relative
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn name_of(relative: &str) -> &str {
    relative
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(relative)
}

fn is_page(name: &str) -> bool {
    matches!(
        extension(name).as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "avif")
    )
}

fn is_sidecar(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "comicinfo.xml" | "metadata.opf" | "cover.jpg" | "cover.png"
    )
}

fn extension(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    if extension.is_empty() || name.starts_with('.') && !name[1..].contains('.') {
        return None;
    }
    Some(extension.to_ascii_lowercase())
}

fn type_name(kind: FileType) -> &'static str {
    match kind {
        FileType::Epub => "epub",
        FileType::Cbz => "cbz",
        FileType::Cb7 => "cb7",
        FileType::Cbt => "cbt",
        FileType::Cbr => "cbr",
        FileType::Pdf => "pdf",
        FileType::Mobi => "mobi",
        FileType::ImageFolder => "image-folder",
        FileType::Unknown => "unknown",
    }
}

fn token_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_owned())
}

fn token_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_owned())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}
