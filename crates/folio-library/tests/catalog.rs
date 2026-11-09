//! Two-pass catalog. Pass 1 must not open an archive or a PDF.

use std::error::Error;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use folio_core::FileType;
use folio_formats::bind_pdfium;
use folio_library::{
    cover_cache_path, identify, open_listed, scan_directory, scan_listing, ScanDepth, ScanError,
    ScanRequest,
};
use folio_platform::list_directory;
use folio_storage::{CatalogItem, Database};
use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
use tar::Header;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[test]
fn identity_lists_before_any_archive_opens() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    write_library(&fixture.root)?;
    let entries = list_directory(&fixture.root)?;
    let rows = identify(&entries, &mut |_| panic!("pass 1 sniffed a file"));
    assert_eq!(rows.len(), 6);
    expect_identity(
        &rows,
        "Dock Seven/issue.cbz",
        FileType::Cbz,
        "issue",
        "Dock Seven",
    );
    expect_identity(
        &rows,
        "Spine Book/book.epub",
        FileType::Epub,
        "book",
        "Spine Book",
    );
    expect_identity(&rows, "Packed/book.cb7", FileType::Cb7, "book", "Packed");
    expect_identity(&rows, "Bundled/book.cbt", FileType::Cbt, "book", "Bundled");
    expect_identity(&rows, "Scans/page.pdf", FileType::Pdf, "page", "Scans");
    let gallery = rows
        .iter()
        .find(|row| row.source == "Gallery")
        .expect("gallery");
    assert_eq!(gallery.format, FileType::ImageFolder);
    assert_eq!(gallery.label, "Gallery");
    assert_eq!(gallery.series_guess, "Gallery");
    assert!(gallery.size.is_some());
    assert!(gallery.modified_ms.is_some());
    assert_eq!(gallery.page_count, None);
    assert_eq!(gallery.image_child_count, Some(2));
    for row in &rows {
        if row.format != FileType::ImageFolder {
            assert_eq!(row.page_count, None, "{}", row.source);
            assert_eq!(row.image_child_count, None, "{}", row.source);
        }
    }

    let db = fixture.db()?;
    let outcome = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &AtomicBool::new(false),
        ScanDepth::Identity,
    )?;
    assert!(outcome.opened.is_empty());
    assert_eq!(outcome.sniffs, 0);
    assert!(outcome.rows.iter().all(|row| row.page_count.is_none()));
    Ok(())
}

#[test]
fn enrichment_fills_parsed_fields() -> Result<(), Box<dyn Error>> {
    let _pdfium = bind_pdfium(&pdfium_dir()).expect("pdfium library");
    let fixture = Fixture::new()?;
    write_library(&fixture.root)?;
    let db = fixture.db()?;
    let outcome = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &AtomicBool::new(false),
        ScanDepth::Enrich,
    )?;
    assert_eq!(outcome.opened.len(), 6);
    let cbz = item(&outcome.rows, "Dock Seven/issue.cbz");
    assert_eq!(cbz.series.as_deref(), Some("Parsed Dock"));
    assert_eq!(cbz.series_guess, "Dock Seven");
    assert_eq!(cbz.page_count, Some(1));
    assert!(cover_cache_path(&fixture.covers, &cbz.id).is_file());
    let epub = item(&outcome.rows, "Spine Book/book.epub");
    assert_eq!(epub.title, "Spine Book");
    assert_eq!(epub.page_count, Some(2));
    assert!(cover_cache_path(&fixture.covers, &epub.id).is_file());
    assert_eq!(item(&outcome.rows, "Packed/book.cb7").page_count, Some(1));
    assert_eq!(item(&outcome.rows, "Bundled/book.cbt").page_count, Some(1));
    assert_eq!(item(&outcome.rows, "Scans/page.pdf").page_count, Some(1));
    assert_eq!(item(&outcome.rows, "Gallery").page_count, Some(2));
    Ok(())
}

#[test]
fn unchanged_tree_opens_nothing() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    write_pair(&fixture.root)?;
    let db = fixture.db()?;
    let cancel = AtomicBool::new(false);
    scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    let again = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    assert!(again.opened.is_empty(), "{:?}", again.opened);
    assert_eq!(again.sniffs, 0);
    Ok(())
}

#[test]
fn one_change_reopens_one_source() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    write_pair(&fixture.root)?;
    fs::create_dir(fixture.root.join("Other"))?;
    fs::write(
        fixture.root.join("Other/else.cbz"),
        cbz_bytes("Parsed Dock")?,
    )?;
    let db = fixture.db()?;
    let cancel = AtomicBool::new(false);
    scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;

    let mut entries = list_directory(&fixture.root)?;
    let changed = entries
        .iter_mut()
        .find(|entry| entry.relative == "Other/else.cbz")
        .expect("else");
    changed.size = changed.size.map(|size| size.saturating_add(1));
    let sized = scan_with(&fixture, &db, &entries, true, &cancel)?;
    assert_eq!(sized.opened, vec!["Other/else.cbz".to_string()]);

    let resynced = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    assert_eq!(resynced.opened, vec!["Other/else.cbz".to_string()]);

    fs::write(
        fixture.root.join("Dock Seven/ComicInfo.xml"),
        br#"<?xml version="1.0"?><ComicInfo><Series>Sidecar Series</Series></ComicInfo>"#,
    )?;
    let added = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    assert_eq!(added.opened, vec!["Dock Seven/issue.cbz".to_string()]);
    assert_eq!(
        item(&added.rows, "Dock Seven/issue.cbz").series.as_deref(),
        Some("Sidecar Series")
    );

    fs::write(
        fixture.root.join("Dock Seven/ComicInfo.xml"),
        br#"<?xml version="1.0"?><ComicInfo><Series>Edited Series</Series></ComicInfo>"#,
    )?;
    let edited = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    assert_eq!(edited.opened, vec!["Dock Seven/issue.cbz".to_string()]);

    fs::remove_file(fixture.root.join("Dock Seven/ComicInfo.xml"))?;
    let removed = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    assert_eq!(removed.opened, vec!["Dock Seven/issue.cbz".to_string()]);

    fs::write(fixture.root.join("Gallery/3.png"), b"three")?;
    let child = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    assert_eq!(child.opened, vec!["Gallery".to_string()]);
    Ok(())
}

#[test]
fn complete_listing_removes_missing_item() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    write_pair(&fixture.root)?;
    let db = fixture.db()?;
    let cancel = AtomicBool::new(false);
    scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Identity,
    )?;
    db.set_progress("Dock Seven/issue.cbz", "page:1")?;
    db.set_progress("Gallery", "page:2")?;
    fs::remove_file(fixture.root.join("Dock Seven/issue.cbz"))?;

    cancel.store(true, Ordering::SeqCst);
    let held = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Identity,
    )?;
    assert!(held.rows.iter().any(|row| row.id == "Dock Seven/issue.cbz"));
    cancel.store(false, Ordering::SeqCst);

    let entries = list_directory(&fixture.root)?;
    let incomplete = scan_with(&fixture, &db, &entries, false, &cancel)?;
    assert!(incomplete
        .rows
        .iter()
        .any(|row| row.id == "Dock Seven/issue.cbz"));

    let removed = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Identity,
    )?;
    assert!(removed
        .rows
        .iter()
        .all(|row| row.id != "Dock Seven/issue.cbz"));
    assert!(removed.rows.iter().any(|row| row.id == "Gallery"));
    assert_eq!(
        db.progress("Dock Seven/issue.cbz")?.as_deref(),
        Some("page:1")
    );
    assert_eq!(db.progress("Gallery")?.as_deref(), Some("page:2"));
    Ok(())
}

#[test]
fn interrupted_enrichment_resumes() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.root.join("a"))?;
    fs::create_dir(fixture.root.join("b"))?;
    fs::write(fixture.root.join("a/one.cbz"), cbz_bytes("One")?)?;
    fs::write(fixture.root.join("b/two.cbz"), cbz_bytes("Two")?)?;
    let db = fixture.db()?;
    let cancel = AtomicBool::new(false);
    let mut seen = 0u32;
    let entries = list_directory(&fixture.root)?;
    let first = scan_listing(
        &db,
        &ScanRequest {
            entries: &entries,
            complete: true,
            cover_dir: &fixture.covers,
            cancel: &cancel,
            depth: ScanDepth::Enrich,
        },
        &mut |_| None,
        &mut |relative, kind| {
            let parsed = open_listed(&fixture.root, relative, kind)?;
            seen = seen.saturating_add(1);
            if seen == 1 {
                cancel.store(true, Ordering::SeqCst);
            }
            Ok(parsed)
        },
    )?;
    assert_eq!(first.opened, vec!["a/one.cbz".to_string()]);
    assert!(item(&first.rows, "a/one.cbz").page_count.is_some());
    assert_eq!(item(&first.rows, "b/two.cbz").page_count, None);
    assert!(item(&db.catalog()?, "a/one.cbz").page_count.is_some());

    cancel.store(false, Ordering::SeqCst);
    let second = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    assert_eq!(second.opened, vec!["b/two.cbz".to_string()]);

    let mut changed = list_directory(&fixture.root)?;
    let row = changed
        .iter_mut()
        .find(|entry| entry.relative == "a/one.cbz")
        .expect("one");
    row.modified_ms = row.modified_ms.map(|value| value.saturating_add(1));
    let blocked = scan_listing(
        &db,
        &ScanRequest {
            entries: &changed,
            complete: true,
            cover_dir: &fixture.covers,
            cancel: &cancel,
            depth: ScanDepth::Enrich,
        },
        &mut |_| None,
        &mut |_relative, _kind| {
            cancel.store(true, Ordering::SeqCst);
            Err(ScanError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "left the library",
            )))
        },
    )?;
    assert_eq!(blocked.opened, vec!["a/one.cbz".to_string()]);
    cancel.store(false, Ordering::SeqCst);
    let resumed = scan_listing(
        &db,
        &ScanRequest {
            entries: &changed,
            complete: true,
            cover_dir: &fixture.covers,
            cancel: &cancel,
            depth: ScanDepth::Enrich,
        },
        &mut |_| None,
        &mut |relative, kind| open_listed(&fixture.root, relative, kind),
    )?;
    assert_eq!(resumed.opened, vec!["a/one.cbz".to_string()]);
    assert!(item(&resumed.rows, "b/two.cbz")
        .enriched_fingerprint
        .is_some());
    Ok(())
}

#[test]
fn unknown_size_or_mtime_is_not_unchanged() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.root.join("Dock Seven"))?;
    fs::write(
        fixture.root.join("Dock Seven/issue.cbz"),
        cbz_bytes("Parsed Dock")?,
    )?;
    let db = fixture.db()?;
    let cancel = AtomicBool::new(false);
    let base = list_directory(&fixture.root)?;
    for (clear_size, clear_mtime) in [(true, false), (false, true), (true, true)] {
        let mut entries = base.clone();
        for entry in &mut entries {
            if entry.relative == "Dock Seven/issue.cbz" {
                if clear_size {
                    entry.size = None;
                }
                if clear_mtime {
                    entry.modified_ms = None;
                }
            }
        }
        let identified = identify(&entries, &mut |_| panic!("sniff"));
        let book = identified
            .iter()
            .find(|row| row.source == "Dock Seven/issue.cbz")
            .expect("book");
        if clear_size {
            assert_eq!(book.size, None);
        }
        if clear_mtime {
            assert_eq!(book.modified_ms, None);
        }
        scan_with(&fixture, &db, &entries, true, &cancel)?;
        let again = scan_with(&fixture, &db, &entries, true, &cancel)?;
        assert_eq!(again.opened, vec!["Dock Seven/issue.cbz".to_string()]);
    }
    Ok(())
}

#[test]
fn missing_cover_cache_reopens_that_row_only() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    write_pair(&fixture.root)?;
    let db = fixture.db()?;
    let cancel = AtomicBool::new(false);
    let first = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    let before = item(&first.rows, "Dock Seven/issue.cbz").clone();
    fs::remove_file(cover_cache_path(&fixture.covers, "Dock Seven/issue.cbz"))?;
    let again = scan_directory(
        &fixture.root,
        &db,
        &fixture.covers,
        &cancel,
        ScanDepth::Enrich,
    )?;
    assert_eq!(again.opened, vec!["Dock Seven/issue.cbz".to_string()]);
    let after = item(&again.rows, "Dock Seven/issue.cbz");
    assert_eq!(after.series, before.series);
    assert_eq!(after.page_count, before.page_count);
    assert_eq!(after.fingerprint, before.fingerprint);
    assert_eq!(after.label, before.label);
    assert!(cover_cache_path(&fixture.covers, "Gallery").is_file());
    Ok(())
}

struct Fixture {
    root: PathBuf,
    covers: PathBuf,
    database: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        let parent = tempfile::tempdir()?.keep();
        let root = parent.join("library");
        let covers = parent.join("covers");
        fs::create_dir(&root)?;
        fs::create_dir(&covers)?;
        Ok(Self {
            database: parent.join("folio.sqlite"),
            covers,
            root,
        })
    }

    fn db(&self) -> Result<Database, Box<dyn Error>> {
        Ok(Database::open(&self.database)?)
    }
}

fn scan_with(
    fixture: &Fixture,
    db: &Database,
    entries: &[folio_platform::ListedPath],
    complete: bool,
    cancel: &AtomicBool,
) -> Result<folio_library::ScanOutcome, ScanError> {
    scan_listing(
        db,
        &ScanRequest {
            entries,
            complete,
            cover_dir: &fixture.covers,
            cancel,
            depth: ScanDepth::Enrich,
        },
        &mut |_| None,
        &mut |relative, kind| open_listed(&fixture.root, relative, kind),
    )
}

fn expect_identity(
    rows: &[folio_library::IdentityRow],
    source: &str,
    format: FileType,
    label: &str,
    series: &str,
) {
    let row = rows
        .iter()
        .find(|row| row.source == source)
        .unwrap_or_else(|| panic!("missing {source}"));
    assert_eq!(row.format, format);
    assert_eq!(row.label, label);
    assert_eq!(row.series_guess, series);
    assert!(row.size.is_some(), "{source}");
    assert!(row.modified_ms.is_some(), "{source}");
    assert_ne!(row.size, Some(0));
}

fn item<'a>(rows: &'a [CatalogItem], source: &str) -> &'a CatalogItem {
    rows.iter()
        .find(|row| row.source == source)
        .unwrap_or_else(|| panic!("missing {source}"))
}

fn pdfium_dir() -> PathBuf {
    std::env::var_os("FOLIO_PDFIUM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/folio-pdfium/lib"))
}

fn write_library(root: &Path) -> Result<(), Box<dyn Error>> {
    write_pair(root)?;
    fs::create_dir(root.join("Spine Book"))?;
    fs::create_dir(root.join("Packed"))?;
    fs::create_dir(root.join("Bundled"))?;
    fs::create_dir(root.join("Scans"))?;
    fs::write(root.join("Spine Book/book.epub"), epub_bytes()?)?;
    fs::write(
        root.join("Packed/book.cb7"),
        cb7_bytes(&[("2.png", b"two")])?,
    )?;
    fs::write(
        root.join("Bundled/book.cbt"),
        cbt_bytes(&[("2.png", b"two")])?,
    )?;
    fs::write(root.join("Scans/page.pdf"), one_page_pdf())?;
    Ok(())
}

fn write_pair(root: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir(root.join("Dock Seven"))?;
    fs::create_dir(root.join("Gallery"))?;
    fs::write(root.join("Dock Seven/issue.cbz"), cbz_bytes("Parsed Dock")?)?;
    fs::write(root.join("Gallery/1.png"), b"one")?;
    fs::write(root.join("Gallery/2.jpg"), b"two")?;
    Ok(())
}

fn cbz_bytes(series: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let info = format!(r#"<?xml version="1.0"?><ComicInfo><Series>{series}</Series></ComicInfo>"#);
    zip_stored(&[("2.jpg", b"page-two"), ("ComicInfo.xml", info.as_bytes())])
}

fn epub_bytes() -> Result<Vec<u8>, Box<dyn Error>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored)?;
        zip.write_all(b"application/epub+zip")?;
        zip.start_file("META-INF/container.xml", deflated)?;
        zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#)?;
        zip.start_file("OEBPS/content.opf", deflated)?;
        zip.write_all(br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Spine Book</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/><item id="cover" href="cover.jpg" media-type="image/jpeg" properties="cover-image"/></manifest><spine><itemref idref="c2"/><itemref idref="c1"/></spine></package>"#)?;
        zip.start_file("OEBPS/c1.xhtml", deflated)?;
        zip.write_all(b"<html><body><p>Hello</p></body></html>")?;
        zip.start_file("OEBPS/c2.xhtml", deflated)?;
        zip.write_all(b"<html><body><p>World</p></body></html>")?;
        zip.start_file("OEBPS/cover.jpg", deflated)?;
        zip.write_all(b"epub-cover")?;
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

fn zip_stored(files: &[(&str, &[u8])]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, bytes) in files {
            zip.start_file(*name, options)?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

fn cb7_bytes(files: &[(&str, &[u8])]) -> Result<Vec<u8>, Box<dyn Error>> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = ArchiveWriter::new(cursor)?;
    writer.set_encrypt_header(false);
    for (name, bytes) in files {
        let mut entry = ArchiveEntry::new_file(name);
        entry.size = u64::try_from(bytes.len())?;
        writer.push_archive_entry(entry, Some(Cursor::new(*bytes)))?;
    }
    Ok(writer.finish()?.into_inner())
}

fn cbt_bytes(files: &[(&str, &[u8])]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut builder = tar::Builder::new(&mut cursor);
        for (name, bytes) in files {
            let mut header = Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(u64::try_from(bytes.len())?);
            header.set_path(name)?;
            header.set_cksum();
            builder.append(&header, *bytes)?;
        }
        builder.finish()?;
    }
    Ok(cursor.into_inner())
}

fn one_page_pdf() -> Vec<u8> {
    let stream = b"BT /F1 24 Tf 40 100 Td (Hi) Tj ET";
    let objects = [
        "1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n".to_string(),
        "2 0 obj<</Type/Pages/Count 1/Kids[3 0 R]>>endobj\n".to_string(),
        "3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj\n".to_string(),
        format!("4 0 obj<</Length {}>>stream\n{}\nendstream\nendobj\n", stream.len(), std::str::from_utf8(stream).expect("ascii")),
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
