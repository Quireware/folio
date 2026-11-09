//! EPUB from a seekable source. Chapter HTML is stripped before it is returned.

use folio_core::SeekableSource;

use crate::archive::zip_files;
use crate::error::FormatError;
use crate::html::strip_chapter_html;
use crate::names::entry_escapes;
use crate::sidecar::{container_rootfile, spine_hrefs};
use crate::Publication;

/// Open an EPUB.
///
/// The source is not a filesystem path, and the archive is not extracted.
///
/// # Errors
///
/// Returns [`FormatError::Drm`] when `META-INF/encryption.xml` is present,
/// [`FormatError::PathEscape`] when an entry leaves the archive root, and
/// [`FormatError::Invalid`] when the container or spine cannot be read.
/// A missing `dc:title` leaves the title empty. A spine id with no manifest
/// item is skipped. The book is still invalid when every spine id is missing.
pub fn open_epub(source: &mut dyn SeekableSource) -> Result<Publication, FormatError> {
    let entries = zip_files(source)?;
    if entries
        .iter()
        .any(|entry| entry.name.eq_ignore_ascii_case("META-INF/encryption.xml"))
    {
        return Err(FormatError::Drm);
    }
    let container = entry_text(&entries, "META-INF/container.xml")?;
    let rootfile = container_rootfile(&container)?;
    if entry_escapes(&rootfile) {
        return Err(FormatError::PathEscape);
    }
    let opf = entry_text(&entries, &rootfile)?;
    let title = crate::sidecar::opf_title(&opf)?;
    let mut chapters = Vec::new();
    for (_idref, href) in spine_hrefs(&opf)? {
        let path = join_opf_href(&rootfile, &href)?;
        let html = entry_text(&entries, &path)?;
        let image = chapter_image(&entries, &rootfile, &html);
        chapters.push(crate::Chapter {
            href: path,
            html: strip_chapter_html(&html),
            image,
        });
    }
    let marked = cover_bytes(&entries, &rootfile, &opf)?;
    let cover = marked.or_else(|| {
        chapters.iter().find_map(|chapter| {
            // clone: the cover outlives the spine item that holds the same picture
            chapter.image.as_ref().map(|page| page.bytes.clone())
        })
    });
    Ok(Publication {
        title,
        chapters,
        cover,
    })
}

fn entry_text(entries: &[crate::archive::ZipEntry], name: &str) -> Result<String, FormatError> {
    let bytes = find_entry(entries, name)
        .map(|entry| entry.bytes.as_slice())
        .ok_or(FormatError::Invalid { kind: "epub" })?;
    let text = std::str::from_utf8(bytes).map_err(|error| {
        tracing::debug!(%error, "epub part was not utf-8");
        FormatError::Invalid { kind: "epub" }
    })?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(text).to_owned())
}

fn join_opf_href(opf_path: &str, href: &str) -> Result<String, FormatError> {
    let href = href.split(['#', '?']).next().unwrap_or(href);
    let href = percent_decode(href);
    if entry_escapes(&href) {
        return Err(FormatError::PathEscape);
    }
    let opf_path = percent_decode(opf_path);
    let base = opf_path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let mut parts = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    for part in href.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.pop().is_none() {
                return Err(FormatError::PathEscape);
            }
            continue;
        }
        if part.contains(':') || part.contains('\0') {
            return Err(FormatError::PathEscape);
        }
        parts.push(part.to_owned());
    }
    Ok(parts.join("/"))
}

fn find_entry<'a>(
    entries: &'a [crate::archive::ZipEntry],
    name: &str,
) -> Option<&'a crate::archive::ZipEntry> {
    let decoded = percent_decode(name);
    entries.iter().find(|entry| {
        entry.name == name || entry.name == decoded || percent_decode(&entry.name) == decoded
    })
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if let (Some(&high), Some(&low)) = (
                bytes.get(index.saturating_add(1)),
                bytes.get(index.saturating_add(2)),
            ) {
                if let Some(value) = hex_pair(high, low) {
                    out.push(value);
                    index = index.saturating_add(3);
                    continue;
                }
            }
        }
        out.push(bytes[index]);
        index = index.saturating_add(1);
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_owned())
}

fn hex_pair(high: u8, low: u8) -> Option<u8> {
    let high = hex_value(high)?;
    let low = hex_value(low)?;
    Some(high.saturating_mul(16).saturating_add(low))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte.saturating_sub(b'0')),
        b'a'..=b'f' => Some(byte.saturating_sub(b'a').saturating_add(10)),
        b'A'..=b'F' => Some(byte.saturating_sub(b'A').saturating_add(10)),
        _ => None,
    }
}

fn cover_bytes(
    entries: &[crate::archive::ZipEntry],
    rootfile: &str,
    opf: &str,
) -> Result<Option<Vec<u8>>, FormatError> {
    let document = crate::sidecar::parse_xml(opf)?;
    let Some(href) = marked_cover_href(&document) else {
        return Ok(None);
    };
    let path = join_opf_href(rootfile, &href)?;
    Ok(find_entry(entries, &path).map(|entry| {
        // clone: the cover is a separate copy of the zip entry
        entry.bytes.clone()
    }))
}

fn marked_cover_href(document: &roxmltree::Document<'_>) -> Option<String> {
    cover_property(document)
        .or_else(|| meta_cover(document))
        .or_else(|| cover_by_id(document))
        .or_else(|| cover_by_name(document))
}

fn cover_property(document: &roxmltree::Document<'_>) -> Option<String> {
    document.descendants().find_map(|node| {
        if !node.has_tag_name("item") {
            return None;
        }
        let properties = node.attribute("properties").unwrap_or("");
        let marked = properties
            .split_whitespace()
            .any(|property| property == "cover-image");
        marked
            .then(|| node.attribute("href").map(str::to_owned))
            .flatten()
    })
}

fn meta_cover(document: &roxmltree::Document<'_>) -> Option<String> {
    let id = document.descendants().find_map(|node| {
        if !node.has_tag_name("meta") {
            return None;
        }
        let name = node.attribute("name")?;
        name.eq_ignore_ascii_case("cover")
            .then(|| node.attribute("content"))
            .flatten()
    })?;
    item_href(document, |node| node.attribute("id") == Some(id))
}

fn cover_by_id(document: &roxmltree::Document<'_>) -> Option<String> {
    item_href(document, |node| {
        node.attribute("id")
            .is_some_and(|id| id.eq_ignore_ascii_case("cover"))
            && is_image_item(node)
    })
}

fn cover_by_name(document: &roxmltree::Document<'_>) -> Option<String> {
    item_href(document, |node| {
        node.attribute("href").is_some_and(href_names_cover) && is_image_item(node)
    })
}

fn item_href(
    document: &roxmltree::Document<'_>,
    accept: impl Fn(&roxmltree::Node<'_, '_>) -> bool,
) -> Option<String> {
    document.descendants().find_map(|node| {
        if !node.has_tag_name("item") || !accept(&node) {
            return None;
        }
        node.attribute("href").map(str::to_owned)
    })
}

fn is_image_item(node: &roxmltree::Node<'_, '_>) -> bool {
    let media = node.attribute("media-type").unwrap_or("");
    if media.len() >= 6 && media.as_bytes()[..6].eq_ignore_ascii_case(b"image/") {
        return true;
    }
    node.attribute("href")
        .is_some_and(crate::names::is_page_image)
}

fn href_names_cover(href: &str) -> bool {
    let file = href.rsplit(['/', '\\']).next().unwrap_or(href);
    let stem = file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(file);
    stem.to_ascii_lowercase().contains("cover")
}

fn chapter_image(
    entries: &[crate::archive::ZipEntry],
    rootfile: &str,
    html: &str,
) -> Option<crate::Page> {
    let src = first_img_src(html)?;
    if src_has_scheme(src) {
        return None;
    }
    let path = join_opf_href(rootfile, src).ok()?;
    let entry = find_entry(entries, &path)?;
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path.as_str())
        .to_owned();
    Some(crate::Page {
        name,
        // clone: the chapter picture is kept after the zip entries are dropped
        bytes: entry.bytes.clone(),
    })
}

fn src_has_scheme(src: &str) -> bool {
    let head = src.split(['/', '\\', '?', '#']).next().unwrap_or(src);
    head.contains(':')
}

fn first_img_src(html: &str) -> Option<&str> {
    let folded = html.to_ascii_lowercase();
    let folded_bytes = folded.as_bytes();
    let mut index = 0;
    while index < folded_bytes.len() {
        let rest = folded_bytes.get(index..).unwrap_or_default();
        if rest.starts_with(b"<img") {
            let after = index.saturating_add(4);
            if matches!(
                folded_bytes.get(after),
                Some(b'/' | b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
            ) {
                if let Some(end) = folded_bytes
                    .get(index..)
                    .and_then(|tail| tail.iter().position(|byte| *byte == b'>'))
                    .map(|offset| index.saturating_add(offset))
                {
                    if let Some(tag) = html.get(index..=end) {
                        if let Some(src) = attr_value(tag, "src") {
                            return Some(src);
                        }
                    }
                }
            }
        }
        index = index.saturating_add(1);
    }
    None
}

fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let folded = tag.to_ascii_lowercase();
    let mut needle = String::with_capacity(name.len().saturating_add(1));
    needle.push_str(name);
    needle.push('=');
    let start = folded.find(&needle)?;
    let rest = tag.get(start.saturating_add(needle.len())..)?;
    let quote = *rest.as_bytes().first()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let value = rest.get(1..)?;
    let end = value.bytes().position(|byte| byte == quote)?;
    value.get(..end)
}
