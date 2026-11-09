//! Typography, themes, and tap zones for the EPUB reader.
//!
//! A tap zone is an action, not a boolean. User CSS is text and cannot add a script.

use thiserror::Error;

use crate::{Cfi, ReadingMode};

/// Why reader CSS was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CssError {
    /// User CSS contained a script element.
    #[error("user css cannot contain a script")]
    Script,
    /// The font size is outside 1..=96.
    #[error("font size is outside 1..=96")]
    FontSize,
    /// The font family contains a character CSS would treat as syntax.
    #[error("font family contains css syntax")]
    FontFamily,
}

/// Named colour scheme. Custom CSS is passed beside the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Black text on white.
    Light,
    /// Brown text on a warm page.
    Sepia,
    /// Light text on a dark page.
    Dark,
    /// Light text on black.
    Amoled,
    /// Caller-supplied CSS, still after the publisher sheet.
    Custom,
}

/// Font, size, line height, and margin. Size is pixels. Line height is a percent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typography {
    /// CSS font-family value, without quotes or semicolons.
    pub font_family: String,
    /// Font size in CSS pixels.
    pub size_px: u32,
    /// Line height as a percent of the font size. `150` is 1.5.
    pub line_height_percent: u32,
    /// Page margin in CSS pixels.
    pub margin_px: u32,
}

/// What a tap region does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneAction {
    /// Previous page or scroll position.
    Previous,
    /// Next page or scroll position.
    Next,
    /// Open the reader menu.
    Menu,
    /// Ignore the tap.
    Nothing,
}

/// Three regions. Each region names an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapZones {
    /// The left third before direction is applied.
    pub left: ZoneAction,
    /// The centre third.
    pub center: ZoneAction,
    /// The right third before direction is applied.
    pub right: ZoneAction,
}

impl TapZones {
    /// Left goes back, the centre opens the menu, right goes forward.
    pub const EDGES: Self = Self {
        left: ZoneAction::Previous,
        center: ZoneAction::Menu,
        right: ZoneAction::Next,
    };
}

/// One open EPUB. Font size is not part of the stored CFI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpubView {
    cfi: Cfi,
    font_size_px: u32,
    mode: ReadingMode,
}

impl EpubView {
    /// Open `cfi` at `font_size_px` in `mode`.
    ///
    /// # Errors
    ///
    /// Returns an error when `font_size_px` is `0` or greater than 96.
    pub fn new(cfi: Cfi, font_size_px: u32, mode: ReadingMode) -> Result<Self, CssError> {
        if !(1..=96).contains(&font_size_px) {
            return Err(CssError::FontSize);
        }
        Ok(Self {
            cfi,
            font_size_px,
            mode,
        })
    }

    /// Change the font size. The CFI is the same value.
    ///
    /// # Errors
    ///
    /// Returns an error when `font_size_px` is `0` or greater than 96.
    pub fn set_font_size(&mut self, font_size_px: u32) -> Result<(), CssError> {
        if !(1..=96).contains(&font_size_px) {
            return Err(CssError::FontSize);
        }
        self.font_size_px = font_size_px;
        Ok(())
    }

    /// Stored position.
    pub fn cfi(&self) -> &Cfi {
        &self.cfi
    }

    /// Current font size in pixels.
    pub fn font_size_px(&self) -> u32 {
        self.font_size_px
    }

    /// Current reading mode.
    pub fn mode(&self) -> ReadingMode {
        self.mode
    }
}

/// Positions the reader can return to. The newest push is the first pop.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BackStack {
    frames: Vec<Cfi>,
}

impl BackStack {
    /// Remember `cfi`.
    pub fn push(&mut self, cfi: Cfi) {
        self.frames.push(cfi);
    }

    /// The most recently pushed CFI.
    pub fn back(&mut self) -> Option<Cfi> {
        self.frames.pop()
    }
}

/// CSS for `theme`. [`Theme::Custom`] returns `custom` unchanged.
pub fn theme_css(theme: Theme, custom: &str) -> &str {
    match theme {
        Theme::Light => "color:#1b1b1b;background:#ffffff;",
        Theme::Sepia => "color:#3b2f24;background:#f4ecd8;",
        Theme::Dark => "color:#e6e6e6;background:#1c1c1c;",
        Theme::Amoled => "color:#f2f2f2;background:#000000;",
        Theme::Custom => custom,
    }
}

/// Horizontal alignment of chapter text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    /// The start edge. Left in a left-to-right book.
    Start,
    /// Centred lines.
    Center,
    /// Justified lines.
    Justify,
}

/// CSS for paragraph alignment and first-line indent.
pub fn paragraph_css(align: TextAlign, indent: bool) -> &'static str {
    match (align, indent) {
        (TextAlign::Start, false) => "text-align:start;text-indent:0;",
        (TextAlign::Start, true) => "text-align:start;text-indent:1.5em;",
        (TextAlign::Center, false) => "text-align:center;text-indent:0;",
        (TextAlign::Center, true) => "text-align:center;text-indent:1.5em;",
        (TextAlign::Justify, false) => "text-align:justify;text-indent:0;",
        (TextAlign::Justify, true) => "text-align:justify;text-indent:1.5em;",
    }
}

/// The font to draw. An empty server font, or a reader that is not following it, uses `fallback`.
pub fn reader_font<'a>(
    follow_server: bool,
    server_font: Option<&'a str>,
    fallback: &'a str,
) -> &'a str {
    match server_font {
        Some(name) if follow_server && !name.is_empty() => name,
        _ => fallback,
    }
}

/// CSS declarations for `typography`.
///
/// # Errors
///
/// Returns an error when the font family contains `<`, `;`, `{`, or `}`.
pub fn typography_css(typography: &Typography) -> Result<String, CssError> {
    if typography
        .font_family
        .contains(['<', ';', '{', '}', '"', '\''])
    {
        return Err(CssError::FontFamily);
    }
    Ok(format!(
        "font-family:{};font-size:{}px;line-height:{}%;margin:{}px;",
        typography.font_family,
        typography.size_px,
        typography.line_height_percent,
        typography.margin_px
    ))
}

/// Publisher CSS, then theme CSS, then user CSS.
///
/// # Errors
///
/// Returns an error when any sheet contains `<script`, ignoring ASCII case.
pub fn stack_css(publisher: &str, theme: &str, user: &str) -> Result<String, CssError> {
    if [publisher, theme, user]
        .iter()
        .any(|sheet| sheet.to_ascii_lowercase().contains("<script"))
    {
        return Err(CssError::Script);
    }
    Ok(format!("{publisher}\n{theme}\n{user}"))
}
