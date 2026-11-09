//! ComicInfo and OPF fields read from bytes already in memory.

use crate::error::FormatError;

/// `<Series>` text from ComicInfo, when the XML parses and the element is non-empty.
pub fn comic_info_series(xml: &str) -> Option<String> {
    let document = roxmltree::Document::parse(xml).ok()?;
    element_text(&document, "Series")
}

pub(crate) fn opf_title(xml: &str) -> Result<String, FormatError> {
    let document = parse_xml(xml)?;
    Ok(element_text(&document, "title").unwrap_or_default())
}

pub(crate) fn container_rootfile(xml: &str) -> Result<String, FormatError> {
    let document = parse_xml(xml)?;
    document
        .descendants()
        .find(|node| node.has_tag_name("rootfile"))
        .and_then(|node| node.attribute("full-path"))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .ok_or(FormatError::Invalid { kind: "epub" })
}

pub(crate) fn parse_xml(xml: &str) -> Result<roxmltree::Document<'_>, FormatError> {
    roxmltree::Document::parse(xml).map_err(|error| {
        tracing::debug!(%error, "xml rejected");
        FormatError::Invalid { kind: "xml" }
    })
}

fn element_text(document: &roxmltree::Document<'_>, name: &str) -> Option<String> {
    document
        .descendants()
        .find(|node| node.has_tag_name(name))
        .and_then(|node| node.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Spine `idref` values in document order, paired with manifest `href`s.
pub(crate) fn spine_hrefs(xml: &str) -> Result<Vec<(String, String)>, FormatError> {
    let document = parse_xml(xml)?;
    let mut manifest = Vec::new();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("item"))
    {
        let Some(id) = node.attribute("id") else {
            continue;
        };
        let Some(href) = node.attribute("href") else {
            continue;
        };
        manifest.push((id.to_owned(), href.to_owned()));
    }
    let mut spine = Vec::new();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("itemref"))
    {
        let idref = node
            .attribute("idref")
            .ok_or(FormatError::Invalid { kind: "epub" })?;
        let Some(href) = manifest
            .iter()
            .find(|(id, _)| id == idref)
            .map(|(_, href)| {
                // clone: the spine list owns each href after the manifest is dropped
                href.clone()
            })
        else {
            tracing::debug!(idref, "spine item is not in the manifest");
            continue;
        };
        spine.push((idref.to_owned(), href));
    }
    if spine.is_empty() {
        return Err(FormatError::Invalid { kind: "epub" });
    }
    Ok(spine)
}
