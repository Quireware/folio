//! Served chapter HTML cannot carry a script, a form, or a remote image.

use folio_formats::sanitize_chapter_html;

#[test]
fn served_chapter_has_no_script() {
    let raw = r#"<p class="x" onclick="alert(1)">Hello</p><img src="pic.png"><img src="https://example.test/a.png"><script>alert(1)</script><form action="https://example.test"><button>Go</button></form>"#;
    let clean = sanitize_chapter_html(raw);
    let folded = clean.to_ascii_lowercase();
    assert!(!folded.contains("<script"), "{clean}");
    assert!(!folded.contains("onclick"), "{clean}");
    assert!(!folded.contains("<form"), "{clean}");
    assert!(!folded.contains("example.test"), "{clean}");
    assert!(!folded.contains("http"), "{clean}");
    assert!(!folded.contains("dioxus"), "{clean}");
    assert!(!folded.contains("addjavascriptinterface"), "{clean}");
    assert!(!folded.contains("dataset.folio"), "{clean}");
    assert!(clean.contains("Hello"), "{clean}");
    assert!(clean.contains("pic.png"), "{clean}");
}
