//! Format parsers.
//!
//! Each default-feature parser reads a [`SeekableSource`](folio_core::SeekableSource)
//! (or, for an image folder, a list of named slices) and returns owned pages.
//! Archives are not extracted to the filesystem. CBR is decoded in Rust.
//! The `cbr` feature stays off and does not link UnRAR.
#![deny(missing_docs)]

mod archive;
mod epub;
mod error;
mod html;
mod images;
mod mobi;
mod names;
mod pdf;
mod pdf_pages;
mod reader;
mod sidecar;

pub use archive::{open_cb7, open_cbr, open_cbt, open_cbz};
pub use epub::open_epub;
pub use error::FormatError;
pub use html::{sanitize_chapter_html, sanitize_kavita_html};
pub use images::{open_image_folder, ImageEntry};
pub use mobi::{open_mobi, MobiBook};
pub use pdf::{bind_pdfium, bound_pdfium, open_pdf, PdfBook, PdfRenderer, PDF_RENDER_DPI};
pub use pdf_pages::{pdf_page_count, render_pdf_png, render_pdf_png_scaled};
pub use sidecar::comic_info_series;

/// One comic page, held in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Archive or folder name of the page.
    pub name: String,
    /// Encoded image bytes.
    pub bytes: Vec<u8>,
}

/// A comic opened from CBZ, CB7, CBT, or an image folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comic {
    /// `<Series>` from ComicInfo.xml when that file was inside the archive.
    pub series: Option<String>,
    /// Images in natural order. `2` sorts before `10`.
    pub pages: Vec<Page>,
    /// Sidecar cover when one was passed, otherwise the first page.
    pub cover: Option<Vec<u8>>,
}

/// One spine item after script, style, and remote-scheme removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    /// Path of the item inside the EPUB.
    pub href: String,
    /// Chapter markup with scripts, styles, and `http://` / `https://` removed.
    pub html: String,
    /// First local picture in this spine item.
    pub image: Option<Page>,
}

/// An EPUB opened from a seekable source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// `dc:title` when the package has one. Empty when it does not.
    pub title: String,
    /// Spine order, which is not zip order. An id with no manifest item is skipped.
    pub chapters: Vec<Chapter>,
    /// Marked cover, or the first picture in the spine when the package marks none.
    pub cover: Option<Vec<u8>>,
}

/// File extension of `name`, without the dot.
///
/// `name` may be a file name or a path. The last path component is used.
/// A leading-dot name such as `.gitignore` has no extension. The extension
/// is returned with its original case.
pub fn file_extension(name: &str) -> Option<&str> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, extension) = base.rsplit_once('.')?;
    if stem.is_empty() || extension.is_empty() {
        None
    } else {
        Some(extension)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_build_has_no_cbr_feature() {
        assert!(!cfg!(feature = "cbr"));
    }
}
