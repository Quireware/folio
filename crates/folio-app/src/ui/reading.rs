//! One screen of a book. The edges turn the page. The centre opens the menu.

use dioxus::prelude::*;
use folio_reader::Side;

use crate::paginate::{line_px, slice_font_px, TextSlice, PAGE_PADDING_PX};
use crate::session::{page_data_url, OpenedBook};

use super::kavita::{local_font, local_image_style, local_paragraph_css, reader_menu};
use super::{zone_tapped, ModelSignal};

pub(super) fn reading_screen(model: ModelSignal) -> Element {
    let view = {
        let guard = model.read();
        guard.open.as_ref().map(|open| {
            let font_px = guard.font_px;
            let menu = guard.menu;
            let colors = paper_colors(&guard.paper);
            let scroll = guard.reading_mode == "scroll";
            ReaderView {
                title: open.book.title().to_owned(),
                font_px,
                menu,
                colors,
                hide_progress: guard.hide_progress,
                scroll,
                text_css: local_paragraph_css(&guard.align, guard.indent).to_owned(),
                image_css: local_image_style(&guard.fit, guard.zoom),
                font_family: local_font(guard.follow_font, "serif").to_owned(),
                frame: frame_of(
                    &open.book,
                    open.index,
                    open.screen,
                    font_px,
                    open.picture.as_deref(),
                    scroll,
                ),
            }
        })
    };
    let Some(view) = view else {
        return rsx! { p { "No book is open." } };
    };
    let progress = progress_label(&view.frame);
    let percent = progress_percent(&view.frame);
    let line = line_px(view.font_px);
    let inset = PAGE_PADDING_PX;
    let colors = view.colors.clone();
    let overflow = if view.scroll { "auto" } else { "hidden" };
    let text_css = view.text_css.clone();
    let font_family = view.font_family.clone();
    rsx! {
        div {
            style: "position:fixed;inset:0;overflow:{overflow};{colors}font-family:{font_family};{text_css}",
            {frame_view(&view.frame, view.font_px, line, inset, &view.image_css, view.scroll)}
            div {
                style: "position:absolute;inset:0;display:flex;z-index:1;",
                button {
                    style: ZONE,
                    onclick: move |_| zone_tapped(model, Side::Left),
                    " "
                }
                button {
                    style: ZONE,
                    onclick: move |_| zone_tapped(model, Side::Center),
                    " "
                }
                button {
                    style: ZONE,
                    onclick: move |_| zone_tapped(model, Side::Right),
                    " "
                }
            }
            if view.menu {
                {reader_menu(model, &view.title, &progress, false)}
            } else if percent > 0 && !view.hide_progress {
                div {
                    style: "position:absolute;left:0;right:0;bottom:0;height:4px;z-index:2;pointer-events:none;",
                    div { style: "height:100%;width:{percent}%;background:currentColor;" }
                }
            }
        }
    }
}

struct ReaderView {
    title: String,
    font_px: u32,
    menu: bool,
    colors: String,
    hide_progress: bool,
    scroll: bool,
    text_css: String,
    image_css: String,
    font_family: String,
    frame: Frame,
}

enum Frame {
    Text {
        slices: Vec<TextSlice>,
        screen: usize,
        pages: usize,
    },
    Image {
        url: String,
        index: usize,
        len: usize,
    },
    Empty,
}

fn frame_of(
    book: &OpenedBook,
    index: usize,
    screen: usize,
    font_px: u32,
    pdf_url: Option<&str>,
    scroll: bool,
) -> Frame {
    match book {
        OpenedBook::Text { pictures, .. } => {
            let pages = crate::paginate::paginate(
                book.blocks(index),
                crate::paginate::boox_window(font_px),
            );
            if pages.is_empty() {
                return pictures.get(index).and_then(Option::as_ref).map_or(
                    Frame::Empty,
                    |picture| Frame::Image {
                        url: page_data_url(&picture.name, &picture.bytes),
                        index,
                        len: book.len(),
                    },
                );
            }
            if scroll {
                let slices = pages
                    .into_iter()
                    .flat_map(|page| page.slices)
                    .collect::<Vec<_>>();
                return Frame::Text {
                    slices,
                    screen: index,
                    pages: book.len().max(1),
                };
            }
            let screen = screen.min(pages.len().saturating_sub(1));
            // clone: this screen is drawn after the chapter borrow ends
            let slices = pages[screen].slices.clone();
            Frame::Text {
                slices,
                screen,
                pages: pages.len(),
            }
        }
        OpenedBook::Pages { pages, .. } => {
            pages.get(index).map_or(Frame::Empty, |page| Frame::Image {
                url: page_data_url(&page.name, &page.bytes),
                index,
                len: pages.len(),
            })
        }
        OpenedBook::Pdf { page_count, .. } => pdf_url.map_or(Frame::Empty, |url| Frame::Image {
            url: url.to_owned(),
            index,
            len: *page_count,
        }),
    }
}

fn frame_view(
    frame: &Frame,
    font_px: u32,
    line: u32,
    inset: u32,
    image_css: &str,
    scroll: bool,
) -> Element {
    let overflow = if scroll { "auto" } else { "hidden" };
    match frame {
        Frame::Text { slices, .. } => {
            let rows = slices
                .iter()
                .enumerate()
                .map(|(index, slice)| {
                    let size = slice_font_px(slice.kind, font_px);
                    let gap = if index == 0 { 0 } else { line };
                    let style = format!(
                        "white-space:pre-wrap;margin:{gap}px 0 0;font-size:{size}px;line-height:150%;"
                    );
                    (index, style, slice.text.clone())
                })
                .collect::<Vec<_>>();
            rsx! {
                div {
                    style: "position:absolute;inset:0;padding:{inset}px;box-sizing:border-box;overflow:{overflow};pointer-events:none;",
                    for (index, style, text) in rows {
                        div { key: "{index}", style: "{style}", "{text}" }
                    }
                }
            }
        }
        Frame::Image { url, .. } => rsx! {
            div {
                style: "position:absolute;inset:0;overflow:{overflow};",
                img {
                    style: "{image_css}",
                    src: "{url}",
                    alt: "Page",
                }
            }
        },
        Frame::Empty => rsx! {
            p { style: "padding:28px;", "This book has no pages." }
        },
    }
}

fn progress_label(frame: &Frame) -> String {
    match frame {
        Frame::Text { screen, pages, .. } => format!("{} / {pages}", screen.saturating_add(1)),
        Frame::Image { index, len, .. } => format!("{} / {len}", index.saturating_add(1)),
        Frame::Empty => "0 / 0".to_owned(),
    }
}

fn progress_percent(frame: &Frame) -> usize {
    let (shown, total) = match frame {
        Frame::Text { screen, pages, .. } => (screen.saturating_add(1), *pages),
        Frame::Image { index, len, .. } => (index.saturating_add(1), *len),
        Frame::Empty => (0, 0),
    };
    if total == 0 {
        0
    } else {
        shown.saturating_mul(100) / total
    }
}

fn paper_colors(paper: &str) -> String {
    let theme = match paper {
        "sepia" => folio_reader::Theme::Sepia,
        "dark" => folio_reader::Theme::Dark,
        "amoled" => folio_reader::Theme::Amoled,
        _ => folio_reader::Theme::Light,
    };
    folio_reader::theme_css(theme, "").to_owned()
}

const ZONE: &str = "flex:1;min-width:48px;height:100%;margin:0;padding:0;border:0;outline:none;background:transparent;color:transparent;";
