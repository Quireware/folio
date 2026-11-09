//! PDF via a dynamically loaded Pdfium. Raster calls take one process-wide lock.
//!
//! Pdfium itself is not linked into this crate. Callers pass the directory that
//! contains `libpdfium.so` (or the platform equivalent). This module only loads
//! that library. The F-Droid artefact has to be built from the pinned source,
//! not from a vendored binary.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};

use folio_core::SeekableSource;
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

use crate::error::FormatError;

/// Product raster density for a PDF page.
pub const PDF_RENDER_DPI: u16 = 150;

static RENDERER: OnceLock<PdfRenderer> = OnceLock::new();
static BIND: Mutex<()> = Mutex::new(());

/// One Pdfium for the process. Renders do not overlap.
pub struct PdfRenderer {
    pdfium: Mutex<Pdfium>,
    hold: Mutex<bool>,
    hold_changed: Condvar,
    entered: AtomicU32,
}

/// Page count and outline titles from one PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfBook {
    /// Number of pages.
    pub page_count: u32,
    /// Bookmark titles in outline order. Empty when the file has none.
    pub outline: Vec<String>,
}

/// Load Pdfium from `library_dir` once.
///
/// Later calls return the same renderer and ignore a different directory.
/// Pdfium's own loader accepts a single binding per process.
///
/// # Errors
///
/// Returns [`FormatError::PdfUnavailable`] when the shared library is missing.
pub fn bind_pdfium(library_dir: &Path) -> Result<&'static PdfRenderer, FormatError> {
    if let Some(renderer) = RENDERER.get() {
        return Ok(renderer);
    }
    let _guard = match BIND.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(renderer) = RENDERER.get() {
        return Ok(renderer);
    }
    let renderer = PdfRenderer::bind(library_dir)?;
    // The bind lock is held, so a failed set means this call lost a race that
    // the lock was meant to prevent. Drop the duplicate and return the stored one.
    if let Err(duplicate) = RENDERER.set(renderer) {
        drop(duplicate);
    }
    RENDERER.get().ok_or(FormatError::PdfUnavailable)
}

impl PdfRenderer {
    fn bind(library_dir: &Path) -> Result<Self, FormatError> {
        let path = Pdfium::pdfium_platform_library_name_at_path(library_dir);
        let bindings = Pdfium::bind_to_library(&path).map_err(|error| {
            tracing::debug!(%error, ?path, "pdfium did not load");
            FormatError::PdfUnavailable
        })?;
        Ok(Self {
            pdfium: Mutex::new(Pdfium::new(bindings)),
            hold: Mutex::new(false),
            hold_changed: Condvar::new(),
            entered: AtomicU32::new(0),
        })
    }

    /// Park the next render inside the render lock until this is set false.
    ///
    /// The default is false. Leaving it true stalls every render, which is how
    /// the overlap test holds the first call.
    pub fn set_render_hold(&self, held: bool) {
        match self.hold.lock() {
            Ok(mut flag) => {
                *flag = held;
                self.hold_changed.notify_all();
            }
            Err(poisoned) => {
                let mut flag = poisoned.into_inner();
                *flag = held;
                self.hold_changed.notify_all();
            }
        }
    }

    /// How many renders have acquired the lock. The overlap test watches this.
    pub fn renders_entered(&self) -> u32 {
        self.entered.load(Ordering::SeqCst)
    }

    /// Read page count and outline titles.
    ///
    /// # Errors
    ///
    /// Returns [`FormatError::Invalid`] when Pdfium rejects the bytes.
    pub fn open_bytes(&self, bytes: &[u8]) -> Result<PdfBook, FormatError> {
        let pdfium = self.lock_render()?;
        read_book(&pdfium, bytes)
    }

    /// Rasterise `index` at `dpi` and return RGBA bytes. The product default is 150.
    ///
    /// # Errors
    ///
    /// Returns [`FormatError::Invalid`] when the page or the raster call fails.
    pub fn render_page(&self, bytes: &[u8], index: u32, dpi: u16) -> Result<Vec<u8>, FormatError> {
        let pdfium = self.lock_render()?;
        let document = pdfium
            .load_pdf_from_byte_slice(bytes, None)
            .map_err(pdf_error)?;
        let page_index = i32::try_from(index).map_err(|_| FormatError::Invalid { kind: "pdf" })?;
        let page = document.pages().get(page_index).map_err(pdf_error)?;
        let width = target_pixels(page.width().value, dpi)?;
        let config = PdfRenderConfig::new().set_target_width(width);
        let bitmap = page.render_with_config(&config).map_err(pdf_error)?;
        let pixels = bitmap.as_rgba_bytes();
        if pixels.is_empty() {
            return Err(FormatError::Invalid { kind: "pdf" });
        }
        Ok(pixels)
    }

    fn lock_render(&self) -> Result<MutexGuard<'_, Pdfium>, FormatError> {
        let guard = self
            .pdfium
            .lock()
            .map_err(|_| FormatError::Invalid { kind: "pdf" })?;
        self.entered.fetch_add(1, Ordering::SeqCst);
        let mut held = self
            .hold
            .lock()
            .map_err(|_| FormatError::Invalid { kind: "pdf" })?;
        while *held {
            held = self
                .hold_changed
                .wait(held)
                .map_err(|_| FormatError::Invalid { kind: "pdf" })?;
        }
        drop(held);
        Ok(guard)
    }
}

/// Renderer from a previous successful [`bind_pdfium`], if this process has one.
///
/// # Errors
///
/// Returns [`FormatError::PdfUnavailable`] when [`bind_pdfium`] has not succeeded.
pub fn bound_pdfium() -> Result<&'static PdfRenderer, FormatError> {
    RENDERER.get().ok_or(FormatError::PdfUnavailable)
}

/// Copy `source` and open it with the renderer from [`bind_pdfium`].
///
/// # Errors
///
/// Returns [`FormatError::PdfUnavailable`] when [`bind_pdfium`] has not succeeded,
/// and [`FormatError::Invalid`] when the file is not a readable PDF.
pub fn open_pdf(source: &mut dyn SeekableSource) -> Result<PdfBook, FormatError> {
    let bytes = read_all(source)?;
    let renderer = bound_pdfium()?;
    renderer.open_bytes(&bytes)
}

fn read_book(pdfium: &Pdfium, bytes: &[u8]) -> Result<PdfBook, FormatError> {
    let document = pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(pdf_error)?;
    let page_count =
        u32::try_from(document.pages().len()).map_err(|_| FormatError::Invalid { kind: "pdf" })?;
    let mut outline = Vec::new();
    for bookmark in document.bookmarks().iter() {
        if let Some(title) = bookmark.title() {
            outline.push(title);
        }
    }
    Ok(PdfBook {
        page_count,
        outline,
    })
}

fn read_all(source: &mut dyn SeekableSource) -> Result<Vec<u8>, FormatError> {
    let len = usize::try_from(source.len()?).map_err(|_| FormatError::Invalid { kind: "pdf" })?;
    let mut bytes = vec![0u8; len];
    let read = source.read_at(0, &mut bytes)?;
    if read != bytes.len() {
        return Err(FormatError::Invalid { kind: "pdf" });
    }
    Ok(bytes)
}

fn target_pixels(points: f32, dpi: u16) -> Result<i32, FormatError> {
    let pixels = (f64::from(points) * f64::from(dpi) / 72.0).round();
    if !pixels.is_finite() || pixels <= 0.0 {
        return Err(FormatError::Invalid { kind: "pdf" });
    }
    format!("{pixels:.0}")
        .parse()
        .map_err(|_| FormatError::Invalid { kind: "pdf" })
}

fn pdf_error(error: impl std::fmt::Display) -> FormatError {
    tracing::debug!(%error, "pdfium rejected a document");
    FormatError::Invalid { kind: "pdf" }
}
