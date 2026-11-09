//! CFI parse, font size, and tap zones.

use folio_reader::{
    format_cfi, jump_to_spine_step, parse_cfi, seek_in_chapter, stack_css, BackStack, EpubView,
    ReadingMode, TapZones, Theme, ZoneAction,
};

#[test]
fn cfi_roundtrip() {
    let raw = "epubcfi(/6/4!/4/2/1:0)";
    let parsed = parse_cfi(raw).expect("cfi");
    assert_eq!(format_cfi(&parsed), raw);
    let asserted = parse_cfi("epubcfi(/6/4[chap]!/4/2:3)").expect("asserted");
    assert_eq!(format_cfi(&asserted), "epubcfi(/6/4[chap]!/4/2:3)");
    assert!(parse_cfi("chapter-2").is_err());
}

#[test]
fn cfi_is_unchanged_when_the_font_size_changes() {
    let cfi = parse_cfi("epubcfi(/6/4!/4/2/1:0)").expect("cfi");
    let mut view = EpubView::new(cfi, 16, ReadingMode::Paginated).expect("view");
    let before = format_cfi(view.cfi());
    view.set_font_size(22).expect("size");
    assert_eq!(format_cfi(view.cfi()), before);
    assert_eq!(view.font_size_px(), 22);
    assert_ne!(ReadingMode::Paginated, ReadingMode::Scroll);
    assert_ne!(view.mode(), ReadingMode::Scroll);

    let sought = seek_in_chapter(view.cfi(), 12);
    assert_eq!(format_cfi(&sought), "epubcfi(/6/4!/4/2/1:12)");
    let jumped = jump_to_spine_step(view.cfi(), 8).expect("jump");
    assert_eq!(format_cfi(&jumped), "epubcfi(/6/8:0)");
    let mut stack = BackStack::default();
    stack.push(parse_cfi(&before).expect("back"));
    assert_eq!(
        stack.back().as_ref().map(format_cfi).as_deref(),
        Some(before.as_str())
    );
}

#[test]
fn cfi_tap_zones_are_actions_and_user_css_cannot_add_a_script() {
    let zones = TapZones::EDGES;
    assert_eq!(zones.left, ZoneAction::Previous);
    assert_eq!(zones.center, ZoneAction::Menu);
    assert_eq!(zones.right, ZoneAction::Next);
    let stacked = stack_css(
        "publisher{}",
        folio_reader::theme_css(Theme::Sepia, ""),
        "user{}",
    )
    .expect("css");
    let mut lines = stacked.lines();
    assert_eq!(lines.next(), Some("publisher{}"));
    assert_eq!(
        lines.next(),
        Some(folio_reader::theme_css(Theme::Sepia, ""))
    );
    assert_eq!(lines.next(), Some("user{}"));
    assert!(stack_css("ok", "ok", "x <script>alert(1)</script>").is_err());
}
