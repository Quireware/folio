//! An image folder is a list of named byte slices, not one seekable stream.
//!
//! A directory has no single [`SeekableSource`](folio_core::SeekableSource).
//! Callers list the children and pass their names and bytes here.

use crate::names::{is_page_image, natural_cmp};
use crate::{Comic, FormatError, Page};

/// One image-folder child.
#[derive(Debug, Clone, Copy)]
pub struct ImageEntry<'a> {
    /// File name relative to the folder.
    pub name: &'a str,
    /// File bytes.
    pub bytes: &'a [u8],
}

/// Keep JPEG, PNG, WebP, and AVIF entries and sort them naturally.
///
/// GIF and other extensions are ignored. `sidecar_cover`, when present,
/// replaces the first page as the cover.
///
/// # Errors
///
/// Returns [`FormatError::PathEscape`] when a name leaves the folder.
pub fn open_image_folder(
    entries: &[ImageEntry<'_>],
    sidecar_cover: Option<&[u8]>,
) -> Result<Comic, FormatError> {
    let mut pages = Vec::new();
    for entry in entries {
        if crate::names::entry_escapes(entry.name) {
            return Err(FormatError::PathEscape);
        }
        if !is_page_image(entry.name) {
            continue;
        }
        pages.push(Page {
            // to_owned: the page name outlives the borrowed entry
            name: entry.name.to_owned(),
            bytes: entry.bytes.to_vec(),
        });
    }
    pages.sort_by(|left, right| natural_cmp(&left.name, &right.name));
    let cover = match sidecar_cover {
        Some(bytes) => Some(bytes.to_vec()),
        // clone: the cover is stored apart from the page that supplied it
        None => pages.first().map(|page| page.bytes.clone()),
    };
    Ok(Comic {
        series: None,
        pages,
        cover,
    })
}
