//! Comic fit, direction, spreads, and the webtoon preload window.
//!
//! Zoom is session state. It is not a setting row.

use thiserror::Error;

use crate::ZoneAction;

/// Pages kept behind the current webtoon page.
pub const WEBTOON_BEHIND: u32 = 1;
/// Pages kept ahead of the current webtoon page.
pub const WEBTOON_AHEAD: u32 = 3;

/// Why a comic layout value was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ComicError {
    /// The series id was empty.
    #[error("series id must not be empty")]
    Series,
}

/// How a page is fitted to the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Match the viewport width.
    Width,
    /// Match the viewport height.
    Height,
    /// The largest scale that keeps the whole page visible.
    Screen,
    /// One image pixel per screen pixel.
    Original,
    /// Width for wide pages, height for tall pages.
    Smart,
}

/// Reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadDirection {
    /// The next page is to the right.
    Ltr,
    /// The next page is to the left.
    Rtl,
}

/// In-memory zoom. Cycling returns to [`SessionZoom::Fit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionZoom {
    /// Use the current [`Fit`].
    Fit,
    /// Twice the fit scale.
    Twice,
    /// Four times the fit scale.
    Four,
}

/// Which third of the page was tapped, in screen space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The left third of the screen.
    Left,
    /// The centre third of the screen.
    Center,
    /// The right third of the screen.
    Right,
}

/// One screen of pages. `second` is absent for a single page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSpread {
    /// First page index, zero-based.
    pub first: u32,
    /// Facing page, when this spread has two.
    pub second: Option<u32>,
}

/// CSS size for one fitted page. Zoom is applied by the caller.
pub fn image_fit_css(fit: Fit) -> &'static str {
    match fit {
        Fit::Width => "width:100%;height:auto;object-fit:contain;",
        Fit::Height => "width:auto;height:100%;object-fit:contain;",
        Fit::Original => "width:auto;height:auto;object-fit:none;",
        Fit::Screen | Fit::Smart => "width:100%;height:100%;object-fit:contain;",
    }
}

/// Scale factor for the session zoom. Fit is 1.
pub fn zoom_factor(zoom: SessionZoom) -> u8 {
    match zoom {
        SessionZoom::Fit => 1,
        SessionZoom::Twice => 2,
        SessionZoom::Four => 4,
    }
}

/// Next zoom step: fit, then 2×, then 4×, then fit.
pub fn cycle_zoom(zoom: SessionZoom) -> SessionZoom {
    match zoom {
        SessionZoom::Fit => SessionZoom::Twice,
        SessionZoom::Twice => SessionZoom::Four,
        SessionZoom::Four => SessionZoom::Fit,
    }
}

/// Setting key for a per-series direction override.
///
/// # Errors
///
/// Returns an error when `series_id` is empty.
pub fn series_direction_key(series_id: &str) -> Result<String, ComicError> {
    if series_id.is_empty() {
        return Err(ComicError::Series);
    }
    Ok(format!("series-direction:{series_id}"))
}

/// Token stored for `direction`.
pub fn direction_token(direction: ReadDirection) -> &'static str {
    match direction {
        ReadDirection::Ltr => "ltr",
        ReadDirection::Rtl => "rtl",
    }
}

/// Parse a stored direction token.
pub fn parse_direction_token(value: &str) -> Option<ReadDirection> {
    match value {
        "ltr" => Some(ReadDirection::Ltr),
        "rtl" => Some(ReadDirection::Rtl),
        _ => None,
    }
}

/// `Manga="YesRTL"` and `<Manga>YesAndRightToLeft</Manga>` are RTL. Anything else is LTR.
pub fn direction_from_comic_info(xml: &str) -> ReadDirection {
    if manga_value(xml).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "yesrtl" | "yesandrighttoleft"
        )
    }) {
        ReadDirection::Rtl
    } else {
        ReadDirection::Ltr
    }
}

/// `stored` replaces the ComicInfo guess, including a Western LTR override of an RTL file.
pub fn resolve_direction(xml: &str, stored: Option<ReadDirection>) -> ReadDirection {
    stored.unwrap_or_else(|| direction_from_comic_info(xml))
}

/// Screen side to action. RTL swaps the left and right zones.
pub fn reading_action(side: Side, direction: ReadDirection, zones: crate::TapZones) -> ZoneAction {
    let side = match (side, direction) {
        (Side::Left, ReadDirection::Rtl) => Side::Right,
        (Side::Right, ReadDirection::Rtl) => Side::Left,
        (side, _) => side,
    };
    match side {
        Side::Left => zones.left,
        Side::Center => zones.center,
        Side::Right => zones.right,
    }
}

/// Page-index change for `action`. Direction does not change the index, only which side maps to it.
pub fn index_delta(action: ZoneAction) -> i32 {
    match action {
        ZoneAction::Next => 1,
        ZoneAction::Previous => -1,
        ZoneAction::Menu | ZoneAction::Nothing => 0,
    }
}

/// `current + delta` when that page exists.
pub fn moved_page(current: u32, page_count: u32, delta: i32) -> Option<u32> {
    if page_count == 0 || delta == 0 {
        return None;
    }
    if delta > 0 {
        let step = u32::try_from(delta).ok()?;
        let next = current.checked_add(step)?;
        (next < page_count).then_some(next)
    } else {
        let step = u32::try_from(delta.checked_neg()?).ok()?;
        current.checked_sub(step)
    }
}

/// Cover-aware spreads leave page 0 alone and pair from page 1.
pub fn cover_aware_spreads(page_count: u32) -> Vec<PageSpread> {
    if page_count == 0 {
        return Vec::new();
    }
    let mut spreads = vec![PageSpread {
        first: 0,
        second: None,
    }];
    let mut page = 1u32;
    while page < page_count {
        let second = page.checked_add(1).filter(|next| *next < page_count);
        spreads.push(PageSpread {
            first: page,
            second,
        });
        page = page.saturating_add(2);
    }
    spreads
}

/// Pages a webtoon reader may request: one behind, the current page, and three ahead.
///
/// The result is in page order. It never includes an index outside `0..page_count`.
pub fn webtoon_window(current: u32, page_count: u32) -> Vec<u32> {
    if page_count == 0 || current >= page_count {
        return Vec::new();
    }
    let start = current.saturating_sub(WEBTOON_BEHIND);
    let last = page_count.saturating_sub(1);
    let end = current.saturating_add(WEBTOON_AHEAD).min(last);
    (start..=end).collect()
}

fn manga_value(xml: &str) -> Option<String> {
    let folded = xml.to_ascii_lowercase();
    if let Some(index) = folded.find("manga=\"") {
        let rest = xml.get(index.saturating_add(7)..)?;
        let end = rest.find('"')?;
        return Some(rest[..end].to_owned());
    }
    let index = folded.find("<manga>")?;
    let rest = xml.get(index.saturating_add(7)..)?;
    let end = rest.to_ascii_lowercase().find("</manga>")?;
    Some(rest[..end].trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        cover_aware_spreads, cycle_zoom, direction_from_comic_info, direction_token, index_delta,
        moved_page, parse_direction_token, reading_action, resolve_direction, series_direction_key,
        webtoon_window, Fit, ReadDirection, SessionZoom, Side, WEBTOON_AHEAD, WEBTOON_BEHIND,
    };
    use crate::{TapZones, ZoneAction};

    #[test]
    fn comic_fit_values_are_distinct() {
        let fits = [
            Fit::Width,
            Fit::Height,
            Fit::Screen,
            Fit::Original,
            Fit::Smart,
        ];
        for (index, fit) in fits.iter().enumerate() {
            assert!(!fits[..index].contains(fit));
        }
        assert_eq!(cycle_zoom(SessionZoom::Fit), SessionZoom::Twice);
        assert_eq!(cycle_zoom(SessionZoom::Twice), SessionZoom::Four);
        assert_eq!(cycle_zoom(SessionZoom::Four), SessionZoom::Fit);
    }

    #[test]
    fn comic_info_yesrtl_is_rtl_until_a_series_override_is_reopened() {
        let rtl = r#"<ComicInfo Manga="YesRTL"></ComicInfo>"#;
        let western = "<ComicInfo><Title>Western</Title></ComicInfo>";
        let element = "<ComicInfo><Manga>YesAndRightToLeft</Manga></ComicInfo>";
        assert_eq!(direction_from_comic_info(rtl), ReadDirection::Rtl);
        assert_eq!(direction_from_comic_info(element), ReadDirection::Rtl);
        assert_eq!(direction_from_comic_info(western), ReadDirection::Ltr);
        assert_eq!(
            resolve_direction(rtl, Some(ReadDirection::Ltr)),
            ReadDirection::Ltr
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("folio.sqlite");
        let key = series_direction_key("series-1").expect("key");
        {
            let db = folio_storage::Database::open(&path).expect("open");
            db.set_setting(&key, direction_token(ReadDirection::Ltr))
                .expect("set");
        }
        let db = folio_storage::Database::open(&path).expect("reopen");
        let settings = db.settings().expect("settings");
        let stored = settings.iter().find_map(|(stored_key, value)| {
            (stored_key == &key).then(|| parse_direction_token(value).expect("token"))
        });
        assert_eq!(resolve_direction(rtl, stored), ReadDirection::Ltr);
        assert!(series_direction_key("").is_err());
    }

    #[test]
    fn comic_cover_spreads_and_webtoon_window() {
        let spreads = cover_aware_spreads(5);
        assert_eq!(spreads[0].first, 0);
        assert_eq!(spreads[0].second, None);
        assert_eq!(spreads[1].first, 1);
        assert_eq!(spreads[1].second, Some(2));
        assert_eq!(spreads[2].first, 3);
        assert_eq!(spreads[2].second, Some(4));

        let page_count = 40;
        let requested = webtoon_window(10, page_count);
        assert_eq!(requested, vec![9, 10, 11, 12, 13]);
        assert!(requested.len() < usize::try_from(page_count).expect("count"));
        assert_eq!(
            u32::try_from(requested.len()).expect("len"),
            WEBTOON_BEHIND + 1 + WEBTOON_AHEAD
        );
        assert!(webtoon_window(0, 0).is_empty());

        let zones = TapZones::EDGES;
        assert_eq!(
            reading_action(Side::Left, ReadDirection::Ltr, zones),
            ZoneAction::Previous
        );
        assert_eq!(
            reading_action(Side::Left, ReadDirection::Rtl, zones),
            ZoneAction::Next
        );
        assert_eq!(
            moved_page(3, page_count, index_delta(ZoneAction::Next)),
            Some(4)
        );
    }
}
