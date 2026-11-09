//! Fit chapter text onto one screen. The reader turns these pages.

use crate::session::TextBlock;

/// CSS pixels the text must fit inside, including the page padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageWindow {
    /// WebView width in CSS pixels.
    pub width_px: u32,
    /// WebView height in CSS pixels.
    pub height_px: u32,
    /// Body font size in CSS pixels.
    pub font_px: u32,
}

/// One screen of a chapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenPage {
    /// Blocks on this screen, in reading order.
    pub slices: Vec<TextSlice>,
}

/// A heading or paragraph fragment. `text` is wrapped with `\n` between lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSlice {
    /// How the reader should set the type.
    pub kind: SliceKind,
    /// Visible text. Newlines are the page's own line breaks.
    pub text: String,
}

/// Heading or body text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceKind {
    /// A heading of this rank, from 1 to 6.
    Heading(u8),
    /// Body text.
    Paragraph,
}

/// Padding inside the page, in CSS pixels. The reader uses the same inset.
pub const PAGE_PADDING_PX: u32 = 28;

/// Window for the Boox Tab Ultra C Pro WebView.
///
/// The panel is 1860×2480 physical pixels at 300 dpi. The WebView sits under
/// the status bar and above the navigation bar, about 992×1222 CSS pixels.
/// The last pixels are left for the progress rule.
pub fn boox_window(font_px: u32) -> PageWindow {
    PageWindow {
        width_px: 992,
        height_px: 1200,
        font_px: font_px.clamp(1, 96),
    }
}

/// Font size for one slice, in CSS pixels.
pub fn slice_font_px(kind: SliceKind, font_px: u32) -> u32 {
    let font_px = font_px.clamp(1, 96);
    match kind {
        SliceKind::Paragraph => font_px,
        SliceKind::Heading(1) => font_px.saturating_add(10),
        SliceKind::Heading(2) => font_px.saturating_add(6),
        SliceKind::Heading(_) => font_px.saturating_add(2),
    }
}

/// Line box for `font_px` at 150% line height.
pub fn line_px(font_px: u32) -> u32 {
    font_px.clamp(1, 96).saturating_mul(3).div_ceil(2).max(1)
}

/// Split `blocks` into screens that fit `window`.
///
/// Every non-whitespace word is kept, in order. A block moves to the next
/// screen instead of being split when the whole block fits there.
pub fn paginate(blocks: &[TextBlock], window: PageWindow) -> Vec<ScreenPage> {
    let font_px = window.font_px.clamp(1, 96);
    let inner_w = window
        .width_px
        .saturating_sub(PAGE_PADDING_PX.saturating_mul(2));
    let inner_h = window
        .height_px
        .saturating_sub(PAGE_PADDING_PX.saturating_mul(2));
    let rows = row_budget(inner_h, line_px(font_px));
    let body_columns = column_budget(inner_w, char_px(font_px));
    let mut pages = Vec::new();
    let mut slices = Vec::new();
    let mut used = 0usize;
    for block in blocks {
        let (kind, text) = block_parts(block);
        let columns = match kind {
            SliceKind::Paragraph => body_columns,
            SliceKind::Heading(_) => column_budget(inner_w, char_px(slice_font_px(kind, font_px))),
        };
        let lines = wrap_lines(text, columns);
        if lines.is_empty() {
            continue;
        }
        let mut offset = 0usize;
        while offset < lines.len() {
            if used >= rows {
                push_page(&mut pages, &mut slices);
                used = 0;
            }
            if used > 0 {
                if used.saturating_add(1) >= rows {
                    push_page(&mut pages, &mut slices);
                    used = 0;
                    continue;
                }
                used = used.saturating_add(1);
            }
            let room = rows.saturating_sub(used).max(1);
            let rest = lines.len().saturating_sub(offset);
            if used > 0 && rest <= rows && rest > room {
                push_page(&mut pages, &mut slices);
                used = 0;
                continue;
            }
            let take = room.min(rest);
            let text = lines[offset..offset.saturating_add(take)].join("\n");
            slices.push(TextSlice { kind, text });
            used = used.saturating_add(take);
            offset = offset.saturating_add(take);
        }
    }
    push_page(&mut pages, &mut slices);
    pages
}

fn push_page(pages: &mut Vec<ScreenPage>, slices: &mut Vec<TextSlice>) {
    if slices.is_empty() {
        return;
    }
    pages.push(ScreenPage {
        slices: std::mem::take(slices),
    });
}

fn block_parts(block: &TextBlock) -> (SliceKind, &str) {
    match block {
        TextBlock::Heading { level, text } => (SliceKind::Heading(*level), text.as_str()),
        TextBlock::Paragraph { text } => (SliceKind::Paragraph, text.as_str()),
    }
}

fn char_px(font_px: u32) -> u32 {
    font_px.saturating_mul(3).div_ceil(5).max(1)
}

fn column_budget(inner_px: u32, char_px: u32) -> usize {
    let width = inner_px.max(char_px);
    let count = width / char_px.max(1);
    usize::try_from(count).unwrap_or(1).max(1)
}

fn row_budget(inner_px: u32, line_px: u32) -> usize {
    let height = inner_px.max(line_px);
    let count = height / line_px.max(1);
    let rows = usize::try_from(count).unwrap_or(1).max(1);
    if rows > 3 {
        rows - 2
    } else {
        rows
    }
}

fn wrap_lines(text: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;
    for word in text.split_whitespace() {
        let word_chars = word.chars().count();
        if word_chars > columns {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let mut rest = word;
            while rest.chars().count() > columns {
                let (head, tail) = split_at_chars(rest, columns);
                lines.push(head.to_owned());
                rest = tail;
            }
            current = rest.to_owned();
            count = rest.chars().count();
            continue;
        }
        let needed = if current.is_empty() {
            word_chars
        } else {
            word_chars.saturating_add(1)
        };
        if count.saturating_add(needed) > columns {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
            count = word_chars;
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            count = count.saturating_add(needed);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn split_at_chars(text: &str, count: usize) -> (&str, &str) {
    match text.char_indices().nth(count) {
        Some((byte, _)) => text.split_at(byte),
        None => (text, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::{paginate, PageWindow};
    use crate::session::TextBlock;

    fn window(width_px: u32, height_px: u32) -> PageWindow {
        PageWindow {
            width_px,
            height_px,
            font_px: 20,
        }
    }

    #[test]
    fn short_paragraph_stays_on_one_page() {
        let blocks = vec![TextBlock::Paragraph {
            text: "Hello".to_owned(),
        }];
        let pages = paginate(&blocks, window(400, 800));
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].slices.len(), 1);
        assert_eq!(pages[0].slices[0].text, "Hello");
    }

    #[test]
    fn long_paragraph_keeps_every_word_in_order() {
        let text = "alpha ".repeat(40);
        let blocks = vec![TextBlock::Paragraph { text }];
        let pages = paginate(&blocks, window(160, 120));
        assert!(pages.len() > 1, "pages: {}", pages.len());
        let mut words = Vec::new();
        for page in &pages {
            for slice in &page.slices {
                words.extend(slice.text.split_whitespace().map(str::to_owned));
            }
        }
        assert_eq!(words, vec!["alpha"; 40]);
    }

    #[test]
    fn heading_moves_intact_when_it_does_not_fit_in_the_remaining_lines() {
        let blocks = vec![
            TextBlock::Paragraph {
                text: "one two three four five six seven eight nine ten".to_owned(),
            },
            TextBlock::Heading {
                level: 1,
                text: "Down the rabbit hole".to_owned(),
            },
        ];
        let pages = paginate(&blocks, window(164, 236));
        let heading_page = pages
            .iter()
            .find(|page| {
                page.slices
                    .iter()
                    .any(|slice| slice.text.contains("rabbit"))
            })
            .expect("heading");
        assert_eq!(heading_page.slices.len(), 1);
        assert!(heading_page.slices[0].text.contains("Down"));
        assert!(heading_page.slices[0].text.contains("hole"));
        assert!(!heading_page.slices[0].text.contains("nine"));
    }

    #[test]
    fn boox_window_clamps_the_font() {
        let window = super::boox_window(0);
        assert_eq!(window.font_px, 1);
        assert_eq!(super::boox_window(20).width_px, 992);
    }
}
