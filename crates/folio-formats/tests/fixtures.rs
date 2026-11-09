//! In-memory fixtures for the default-feature parsers.
//!
//! Nothing here is written as a book file. A scratch directory is asserted
//! empty after a parse so an extractor that used the process temp directory
//! would fail this file.

use std::error::Error;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use folio_core::SliceSource;
use folio_formats::{
    open_cb7, open_cbr, open_cbt, open_cbz, open_epub, open_image_folder, FormatError, ImageEntry,
};
use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
use tar::Header;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[test]
fn epub_spine_order_title_and_stripped_html() -> Result<(), Box<dyn Error>> {
    let bytes = epub_bytes(false)?;
    let scratch = Scratch::new()?;
    let mut source = SliceSource::new(&bytes);
    let book = open_epub(&mut source)?;
    assert!(scratch.is_empty()?);
    assert_eq!(book.title, "Spine Book");
    assert_eq!(
        book.chapters
            .iter()
            .map(|chapter| chapter.href.as_str())
            .collect::<Vec<_>>(),
        vec!["OEBPS/c2.xhtml", "OEBPS/c1.xhtml"]
    );
    for chapter in &book.chapters {
        let folded = chapter.html.to_ascii_lowercase();
        assert!(!folded.contains("<script"), "{folded}");
        assert!(!folded.contains("http"), "{folded}");
        assert!(chapter.html.contains("Hello"));
        assert!(chapter.html.contains("World"));
    }
    Ok(())
}

#[test]
fn epub_without_title_skips_a_dangling_spine_id_and_keeps_the_picture() -> Result<(), Box<dyn Error>>
{
    let page = br#"<html><body><p></p><img src="image%200.jpeg" alt="cover"/></body></html>"#;
    let opf = br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/><item id="page" href="page%201.xhtml" media-type="application/xhtml+xml"/><item id="picture" href="image%200.jpeg" media-type="image/jpeg"/></manifest><spine><itemref idref="cover"/><itemref idref="nav"/><itemref idref="page"/></spine></package>"#;
    let bytes = zip_named(&[
        ("mimetype", b"application/epub+zip", true),
        (
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="EPUB/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            false,
        ),
        ("EPUB/content.opf", opf, false),
        ("EPUB/nav.xhtml", b"<html><body><nav></nav></body></html>", false),
        ("EPUB/page 1.xhtml", page, false),
        ("EPUB/image 0.jpeg", b"scan-cover", false),
    ])?;
    let mut source = SliceSource::new(&bytes);
    let book = open_epub(&mut source)?;
    assert!(book.title.is_empty());
    assert_eq!(
        book.chapters
            .iter()
            .map(|chapter| chapter.href.as_str())
            .collect::<Vec<_>>(),
        vec!["EPUB/nav.xhtml", "EPUB/page 1.xhtml"]
    );
    assert_eq!(book.cover.as_deref(), Some(b"scan-cover".as_slice()));
    assert!(book.chapters[0].image.is_none());
    let image = book.chapters[1].image.as_ref().expect("page picture");
    assert_eq!(image.name, "image 0.jpeg");
    assert_eq!(image.bytes, b"scan-cover");
    Ok(())
}

#[test]
#[ignore = "local sample at /tmp/dragonet.epub"]
fn archive_scan_on_disk_opens() -> Result<(), Box<dyn Error>> {
    let path = std::path::Path::new("/tmp/dragonet.epub");
    if !path.is_file() {
        return Ok(());
    }
    let bytes = std::fs::read(path)?;
    let mut source = SliceSource::new(&bytes);
    let book = open_epub(&mut source)?;
    assert!(book.cover.as_ref().is_some_and(|cover| cover.len() > 1000));
    assert!(book.chapters.len() > 10);
    assert!(book.chapters.iter().any(|chapter| chapter.image.is_some()));
    Ok(())
}

#[test]
fn percent_encoded_parent_in_a_spine_href_is_rejected() -> Result<(), Box<dyn Error>> {
    let opf = br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Escaped</dc:title></metadata><manifest><item id="c1" href="%2e%2e/secret.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#;
    let bytes = zip_named(&[
        ("mimetype", b"application/epub+zip", true),
        (
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            false,
        ),
        ("content.opf", opf, false),
        ("secret.xhtml", b"<html><body><p>no</p></body></html>", false),
    ])?;
    let mut source = SliceSource::new(&bytes);
    let error = open_epub(&mut source).expect_err("escaped href");
    assert!(matches!(error, FormatError::PathEscape));
    Ok(())
}

#[test]
fn drm_epub_is_a_typed_error() -> Result<(), Box<dyn Error>> {
    let bytes = epub_bytes(true)?;
    let mut source = SliceSource::new(&bytes);
    let error = open_epub(&mut source).expect_err("encrypted epub");
    assert!(matches!(error, FormatError::Drm));
    Ok(())
}

#[test]
fn cbz_natural_order_series_count_cover_and_no_extract() -> Result<(), Box<dyn Error>> {
    let bytes = cbz_bytes()?;
    let scratch = Scratch::new()?;
    let mut source = SliceSource::new(&bytes);
    let comic = open_cbz(&mut source, None)?;
    assert!(scratch.is_empty()?);
    assert_eq!(comic.series.as_deref(), Some("Dock Seven"));
    assert_eq!(comic.pages.len(), 2);
    assert_eq!(
        comic
            .pages
            .iter()
            .map(|page| page.name.as_str())
            .collect::<Vec<_>>(),
        vec!["2.jpg", "10.jpg"]
    );
    assert_eq!(comic.pages[0].bytes, b"page-two");
    assert_eq!(comic.pages[1].bytes, b"page-ten");
    assert_eq!(comic.cover.as_deref(), Some(b"page-two".as_slice()));

    let mut source = SliceSource::new(&bytes);
    let overridden = open_cbz(&mut source, Some(b"sidecar-cover"))?;
    assert_eq!(
        overridden.cover.as_deref(),
        Some(b"sidecar-cover".as_slice())
    );
    assert_eq!(overridden.pages[0].bytes, b"page-two");
    Ok(())
}

#[test]
fn zip_parent_dir_is_rejected() -> Result<(), Box<dyn Error>> {
    let bytes = zip_with(&[(b"../evil.jpg", b"nope"), (b"2.jpg", b"page-two")], false)?;
    assert!(bytes.windows(7).any(|window| window == b"../evil"));
    let mut source = SliceSource::new(&bytes);
    let error = open_cbz(&mut source, None).expect_err("escaped path");
    assert!(matches!(error, FormatError::PathEscape));
    Ok(())
}

#[test]
fn cb7_page_count_matches() -> Result<(), Box<dyn Error>> {
    let bytes = cb7_bytes(&[("10.png", b"ten"), ("2.png", b"two")])?;
    let scratch = Scratch::new()?;
    let mut source = SliceSource::new(&bytes);
    let comic = open_cb7(&mut source, None)?;
    assert!(scratch.is_empty()?);
    assert_eq!(comic.pages.len(), 2);
    assert_eq!(comic.pages[0].name, "2.png");
    assert_eq!(comic.pages[0].bytes, b"two");
    Ok(())
}

#[test]
fn cbt_page_count_matches() -> Result<(), Box<dyn Error>> {
    let bytes = cbt_bytes(&[("10.png", b"ten"), ("2.png", b"two")])?;
    let scratch = Scratch::new()?;
    let mut source = SliceSource::new(&bytes);
    let comic = open_cbt(&mut source, None)?;
    assert!(scratch.is_empty()?);
    assert_eq!(comic.pages.len(), 2);
    assert_eq!(comic.pages[0].bytes, b"two");
    Ok(())
}

#[test]
fn tar_parent_dir_is_rejected() -> Result<(), Box<dyn Error>> {
    let bytes = cbt_bytes(&[("../evil.jpg", b"nope"), ("2.jpg", b"page")])?;
    let mut source = SliceSource::new(&bytes);
    let error = open_cbt(&mut source, None).expect_err("escaped path");
    assert!(matches!(error, FormatError::PathEscape));
    Ok(())
}

#[test]
fn image_folder_filters_and_sorts() -> Result<(), Box<dyn Error>> {
    let jpeg = b"jpeg".as_slice();
    let png = b"png".as_slice();
    let webp = b"webp".as_slice();
    let avif = b"avif".as_slice();
    let entries = [
        ImageEntry {
            name: "10.png",
            bytes: png,
        },
        ImageEntry {
            name: "notes.txt",
            bytes: b"no",
        },
        ImageEntry {
            name: "2.jpeg",
            bytes: jpeg,
        },
        ImageEntry {
            name: "anim.gif",
            bytes: b"gif",
        },
        ImageEntry {
            name: "1.WEBP",
            bytes: webp,
        },
        ImageEntry {
            name: "3.avif",
            bytes: avif,
        },
    ];
    let folder = open_image_folder(&entries, Some(b"folder-cover"))?;
    assert_eq!(
        folder
            .pages
            .iter()
            .map(|page| page.name.as_str())
            .collect::<Vec<_>>(),
        vec!["1.WEBP", "2.jpeg", "3.avif", "10.png"]
    );
    assert_eq!(folder.cover.as_deref(), Some(b"folder-cover".as_slice()));
    Ok(())
}

#[test]
fn pipe_is_staged_before_the_parser_sees_it() -> Result<(), Box<dyn Error>> {
    let bytes = cbz_bytes()?;
    let (mut reader, mut writer) = std::io::pipe()?;
    writer.write_all(&bytes)?;
    drop(writer);
    let scratch = Scratch::new()?;
    let limit = u64::try_from(bytes.len())?;
    let cancel = AtomicBool::new(false);
    let mut staged = folio_platform::stage_unseekable(&mut reader, scratch.path(), limit, &cancel)?;
    let comic = open_cbz(&mut staged, None)?;
    assert_eq!(comic.pages.len(), 2);
    assert_eq!(comic.pages[0].bytes, b"page-two");
    Ok(())
}

#[test]
fn default_build_excludes_cbr() {
    assert!(!cfg!(feature = "cbr"));
}

#[test]
fn cbr_reads_stored_pngs_without_unrar() -> Result<(), Box<dyn Error>> {
    let bytes = stored_cbr(&[("10.png", b"page-ten"), ("2.png", b"page-two")]);
    let scratch = Scratch::new()?;
    let mut source = SliceSource::new(&bytes);
    let comic = open_cbr(&mut source, None)?;
    assert!(scratch.is_empty()?);
    assert_eq!(
        comic
            .pages
            .iter()
            .map(|page| (page.name.as_str(), page.bytes.as_slice()))
            .collect::<Vec<_>>(),
        vec![
            ("2.png", b"page-two".as_slice()),
            ("10.png", b"page-ten".as_slice())
        ]
    );
    let mut junk = SliceSource::new(b"Rar!");
    assert!(matches!(
        open_cbr(&mut junk, None),
        Err(FormatError::Invalid { kind: "cbr" })
    ));
    let escaped = stored_cbr(&[("../2.png", b"no")]);
    let mut source = SliceSource::new(&escaped);
    assert!(matches!(
        open_cbr(&mut source, None),
        Err(FormatError::PathEscape)
    ));
    Ok(())
}

fn stored_cbr(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = rar_codec::ArchiveBuilder::new();
    for (name, bytes) in files {
        writer.add_stored(name, bytes).expect("store");
    }
    writer.finish()
}

fn epub_bytes(drm: bool) -> Result<Vec<u8>, Box<dyn Error>> {
    let chapter = b"<html><body><p>Hello</p><a href=\"http://remote.example/a\">link</a><script>var x=\"https://cdn.example/s.js\";</script><style>@import \"https://cdn.example/c.css\";</style><p>World</p></body></html>";
    let mut files: Vec<(&str, &[u8], bool)> = vec![
        ("mimetype", b"application/epub+zip", true),
        (
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            false,
        ),
        (
            "OEBPS/content.opf",
            br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Spine Book</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c2"/><itemref idref="c1"/></spine></package>"#,
            false,
        ),
        ("OEBPS/c1.xhtml", chapter, false),
        ("OEBPS/c2.xhtml", chapter, false),
    ];
    if drm {
        files.push((
            "META-INF/encryption.xml",
            br#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container"></encryption>"#,
            false,
        ));
    }
    zip_named(&files)
}

fn cbz_bytes() -> Result<Vec<u8>, Box<dyn Error>> {
    let info = br#"<?xml version="1.0"?><ComicInfo><Series>Dock Seven</Series></ComicInfo>"#;
    zip_with(
        &[
            (b"10.jpg", b"page-ten"),
            (b"ComicInfo.xml", info),
            (b"2.jpg", b"page-two"),
        ],
        true,
    )
}

fn zip_with(files: &[(&[u8], &[u8])], deflate: bool) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let method = if deflate {
            CompressionMethod::Deflated
        } else {
            CompressionMethod::Stored
        };
        let options = SimpleFileOptions::default().compression_method(method);
        for (name, bytes) in files {
            let name = std::str::from_utf8(name)?;
            zip.start_file(name, options)?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

fn zip_named(files: &[(&str, &[u8], bool)]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        for (name, bytes, stored) in files {
            let method = if *stored {
                CompressionMethod::Stored
            } else {
                CompressionMethod::Deflated
            };
            let options = SimpleFileOptions::default().compression_method(method);
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
    let finished = writer.finish()?;
    Ok(finished.into_inner())
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
            if name.split(['/', '\\']).any(|part| part == "..") {
                // `set_path` refuses `..`. The name field is written directly
                // so the parser is tested against a header the writer API hides.
                header.set_path("placeholder")?;
                let raw = header.as_mut_bytes();
                let name_bytes = name.as_bytes();
                if name_bytes.len() >= 100 {
                    return Err("tar name does not fit the header".into());
                }
                raw[..100].fill(0);
                raw[..name_bytes.len()].copy_from_slice(name_bytes);
            } else {
                header.set_path(name)?;
            }
            header.set_cksum();
            builder.append(&header, *bytes)?;
        }
        builder.finish()?;
    }
    Ok(cursor.into_inner())
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, Box<dyn Error>> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("folio-formats-{nanos}-{}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn is_empty(&self) -> Result<bool, Box<dyn Error>> {
        Ok(std::fs::read_dir(&self.0)?.next().is_none())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("scratch cleanup failed: {error}");
        }
    }
}
