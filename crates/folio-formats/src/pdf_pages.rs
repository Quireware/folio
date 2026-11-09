//! PDF pages drawn without Pdfium. The rasteriser is pure Rust.

use std::sync::Arc;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, RenderSettings};

use crate::error::FormatError;

/// How many pages `bytes` contains.
///
/// # Errors
///
/// Returns [`FormatError::Invalid`] when the file is not a readable PDF.
pub fn pdf_page_count(bytes: &[u8]) -> Result<u32, FormatError> {
    let pdf = load(bytes)?;
    u32::try_from(pdf.pages().len()).map_err(|_| FormatError::Invalid { kind: "pdf" })
}

/// Draw page `index` as a PNG at twice the PDF point size.
///
/// # Errors
///
/// Returns [`FormatError::Invalid`] when the file or the page cannot be drawn.
pub fn render_pdf_png(bytes: &[u8], index: usize) -> Result<Vec<u8>, FormatError> {
    render_pdf_png_scaled(bytes, index, 2.0)
}

/// Draw page `index` as a PNG at `scale` times the PDF point size.
///
/// Shelf covers use a scale below one. The reader uses [`render_pdf_png`].
///
/// # Errors
///
/// Returns [`FormatError::Invalid`] when the file or the page cannot be drawn.
pub fn render_pdf_png_scaled(
    bytes: &[u8],
    index: usize,
    scale: f32,
) -> Result<Vec<u8>, FormatError> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(FormatError::Invalid { kind: "pdf" });
    }
    let pdf = load(bytes)?;
    let page = pdf
        .pages()
        .get(index)
        .ok_or(FormatError::Invalid { kind: "pdf" })?;
    let pixmap = render(
        page,
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
            ..RenderSettings::default()
        },
    );
    pixmap.into_png().map_err(|error| {
        tracing::debug!(%error, "pdf page did not encode");
        FormatError::Invalid { kind: "pdf" }
    })
}

fn load(bytes: &[u8]) -> Result<Pdf, FormatError> {
    // to_vec: hayro keeps the file behind an Arc for the parse
    let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(bytes.to_vec());
    Pdf::new(data).map_err(|error| {
        tracing::debug!(?error, "pdf did not parse");
        FormatError::Invalid { kind: "pdf" }
    })
}

#[cfg(test)]
mod tests {
    use super::{pdf_page_count, render_pdf_png};

    #[test]
    fn one_page_pdf_draws_a_png() {
        let bytes = one_page_pdf();
        assert_eq!(pdf_page_count(&bytes).expect("count"), 1);
        let png = render_pdf_png(&bytes, 0).expect("png");
        assert!(png.starts_with(b"\x89PNG"));
        assert!(render_pdf_png(&bytes, 1).is_err());
        assert!(pdf_page_count(b"%PDF").is_err());
        let cover = super::render_pdf_png_scaled(&bytes, 0, 0.5).expect("cover");
        assert!(cover.starts_with(b"\x89PNG"));
        assert!(cover.len() < png.len());
        assert!(super::render_pdf_png_scaled(&bytes, 0, 0.0).is_err());
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
}
