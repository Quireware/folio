//! One real Pdfium raster, plus proof that a second raster waits for the first.

use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use folio_core::SliceSource;
use folio_formats::{bind_pdfium, open_pdf};

#[test]
fn one_page_pdf_renders_at_150_dpi_without_overlap() {
    let library = pdfium_dir();
    let renderer = bind_pdfium(&library).expect("pdfium library");
    let bytes = one_page_pdf();
    let mut source = SliceSource::new(&bytes);
    let book = open_pdf(&mut source).expect("open pdf");
    assert_eq!(book.page_count, 1);
    assert_eq!(book.outline, vec!["Chapter One".to_string()]);

    let before = renderer.renders_entered();
    renderer.set_render_hold(true);
    let first_bytes = bytes.clone();
    let first = thread::spawn(move || renderer.render_page(&first_bytes, 0, 150));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while renderer.renders_entered() == before {
        assert!(
            std::time::Instant::now() < deadline,
            "first render did not take the lock"
        );
        thread::yield_now();
    }
    let second_bytes = bytes.clone();
    let second = thread::spawn(move || renderer.render_page(&second_bytes, 0, 150));
    thread::sleep(Duration::from_millis(80));
    assert_eq!(
        renderer.renders_entered(),
        before.saturating_add(1),
        "second render overlapped the first"
    );
    renderer.set_render_hold(false);
    let first_image = first
        .join()
        .expect("first render thread")
        .expect("first raster");
    let second_image = second
        .join()
        .expect("second render thread")
        .expect("second raster");
    assert!(!first_image.is_empty());
    assert_eq!(first_image, second_image);
    assert!(renderer.renders_entered() >= before.saturating_add(2));
}

fn pdfium_dir() -> PathBuf {
    env::var_os("FOLIO_PDFIUM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/folio-pdfium/lib"))
}

fn one_page_pdf() -> Vec<u8> {
    let stream = b"BT /F1 24 Tf 40 100 Td (Hi) Tj ET";
    let length = stream.len();
    let stream_text = std::str::from_utf8(stream).expect("ascii stream");
    let objects = [
        "1 0 obj<</Type/Catalog/Pages 2 0 R/Outlines 5 0 R>>endobj\n".to_string(),
        "2 0 obj<</Type/Pages/Count 1/Kids[3 0 R]>>endobj\n".to_string(),
        "3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]/Contents 4 0 R/Resources<</Font<</F1 7 0 R>>>>>>endobj\n".to_string(),
        format!("4 0 obj<</Length {length}>>stream\n{stream_text}\nendstream\nendobj\n"),
        "5 0 obj<</Type/Outlines/Count 1/First 6 0 R/Last 6 0 R>>endobj\n".to_string(),
        "6 0 obj<</Title(Chapter One)/Parent 5 0 R>>endobj\n".to_string(),
        "7 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\n".to_string(),
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
