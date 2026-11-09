//! Shared reader positions and the channel the content frame is allowed to use.
#![deny(missing_docs)]

mod adjust;
mod annotation;
mod cfi;
mod channel;
mod comic;
mod dict;
mod kavita;
mod position;
mod preload;
mod search;
mod style;
mod tts;

pub use adjust::{apply_rgba, AdjustError, ImageAdjust};
pub use annotation::{json_export, markdown_export, AnnotationError, Colour, Highlight};
pub use cfi::{format_cfi, jump_to_spine_step, parse_cfi, seek_in_chapter, Cfi, CfiError, CfiStep};
pub use channel::{accept_content_report, content_document, ChannelError, ContentReport};
pub use comic::{
    cover_aware_spreads, cycle_zoom, direction_from_comic_info, direction_token, image_fit_css,
    index_delta, moved_page, parse_direction_token, reading_action, resolve_direction,
    series_direction_key, webtoon_window, zoom_factor, ComicError, Fit, PageSpread, ReadDirection,
    SessionZoom, Side, WEBTOON_AHEAD, WEBTOON_BEHIND,
};
pub use dict::{lookup, Definition};
pub use kavita::{
    can_open, download_complete, HeldPage, HeldWindow, PageWindow, RemotePage, StoredPage,
};
pub use position::{CfiPosition, PagePosition};
pub use preload::{forward_window, reverse_preload, PreloadPlan};
pub use search::{search_chapters, SearchHit};
pub use style::{
    paragraph_css, reader_font, stack_css, theme_css, typography_css, BackStack, CssError,
    EpubView, TapZones, TextAlign, Theme, Typography, ZoneAction,
};
pub use tts::{Speech, SpeechHandle};

/// How a book is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadingMode {
    /// One screen at a time.
    Paginated,
    /// A continuous scroll.
    Scroll,
    /// Comic pages stacked vertically. Each page is still the page route.
    Webtoon,
}
