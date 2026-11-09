//! Library home: the mark and source switch, a search field, then shelves of covers.

use dioxus::prelude::*;
use folio_storage::CatalogItem;

use crate::shell::{home_shelves, progress_line, search_local, SearchRow, ShelfProgress};

use super::icons::{quire_mark, search_icon};
use super::kavita::{activate_kavita, kavita_screen};
use super::{open_book, pressed, title_of, ModelSignal};

pub(super) fn library_screen(mut model: ModelSignal) -> Element {
    let status = model.read().status.clone();
    let query = model.read().query.clone();
    let kavita_on = model.read().kavita.on;
    let items = model.read().items.clone();
    let progress = model.read().progress.clone();
    let rows = search_rows(&items);
    let matches = search_local(&rows, &query)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let ids = items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
    let shelves = home_shelves(&progress, &ids)
        .into_iter()
        .filter(|shelf| shelf.name != "recently-read" && !shelf.book_ids.is_empty())
        .collect::<Vec<_>>();
    rsx! {
        main {
            header {
                class: "f-header",
                div {
                    class: "f-header-row",
                    div {
                        class: "f-brand",
                        {quire_mark()}
                        span { class: "f-wordmark", "Folio" }
                    }
                    p { class: "f-status", "{status}" }
                }
                div {
                    class: "f-seg f-gap-top",
                    button {
                        class: "f-seg__item",
                        aria_pressed: pressed(!kavita_on),
                        onclick: move |_| model.write().kavita.on = false,
                        "Shelf"
                    }
                    button {
                        class: "f-seg__item",
                        aria_pressed: pressed(kavita_on),
                        onclick: move |_| activate_kavita(model),
                        "Kavita"
                    }
                }
                div {
                    class: "f-search",
                    {search_icon()}
                    input {
                        class: "f-input",
                        value: "{query}",
                        placeholder: "Search",
                        oninput: move |event| model.write().query = event.value(),
                    }
                }
            }
            if kavita_on {
                {kavita_screen(model)}
            } else if !query.is_empty() {
                if matches.is_empty() {
                    p { class: "f-empty", "No matches." }
                } else {
                    {cover_grid(model, &items, matches)}
                }
            } else if items.is_empty() {
                p { class: "f-empty", "No books in Books, Articles, Kids, Download, or Documents." }
            } else {
                for shelf in shelves {
                    {shelf_section(model, &items, &progress, shelf.name, shelf.book_ids)}
                }
            }
        }
    }
}

fn shelf_section(
    model: ModelSignal,
    items: &[CatalogItem],
    progress: &[ShelfProgress],
    name: &str,
    ids: Vec<String>,
) -> Element {
    if name == "continue-reading" {
        let cards = ids
            .into_iter()
            .map(|id| continue_card(model, items, progress, id))
            .collect::<Vec<_>>();
        return rsx! {
            section {
                h2 { class: "f-h2", "Continue reading" }
                div {
                    class: "f-row",
                    for card in cards {
                        {card}
                    }
                }
            }
        };
    }
    rsx! {
        section {
            h2 { class: "f-h2", "Library" }
            {cover_grid(model, items, by_title(items, ids))}
        }
    }
}

fn cover_grid(model: ModelSignal, items: &[CatalogItem], ids: Vec<String>) -> Element {
    let cards = ids
        .into_iter()
        .map(|id| book_card(model, items, id))
        .collect::<Vec<_>>();
    rsx! {
        div {
            class: "f-grid",
            for card in cards {
                {card}
            }
        }
    }
}

fn book_card(model: ModelSignal, items: &[CatalogItem], id: String) -> Element {
    let title = title_of(items, &id);
    let kind = kind_label(items, &id);
    let cover = cover_url(model, &id);
    let key = id.clone();
    rsx! {
        button {
            key: "{key}",
            class: "f-book",
            onclick: move |_| open_book(model, id.clone()),
            {cover_box(cover, &title)}
            span { class: "f-book__title", "{title}" }
            span { class: "f-meta", "{kind}" }
        }
    }
}

fn continue_card(
    model: ModelSignal,
    items: &[CatalogItem],
    progress: &[ShelfProgress],
    id: String,
) -> Element {
    let title = title_of(items, &id);
    let kind = kind_label(items, &id);
    let line = items.iter().find(|item| item.id == id).and_then(|item| {
        let row = progress.iter().find(|row| row.book_id == item.id)?;
        progress_line(&item.file_type, row.page, item.page_count)
    });
    let cover = cover_url(model, &id);
    let key = id.clone();
    rsx! {
        button {
            key: "{key}",
            class: "f-continue",
            onclick: move |_| open_book(model, id.clone()),
            {cover_box(cover, &title)}
            span {
                class: "f-continue__body",
                span { class: "f-book__title", "{title}" }
                span { class: "f-meta", "{kind}" }
                if let Some(line) = line {
                    span {
                        class: "f-progress",
                        span { class: "f-progress__fill", style: "width:{line.percent}%;" }
                    }
                    span { class: "f-meta", "{line.label}" }
                }
            }
        }
    }
}

fn cover_box(cover: Option<String>, title: &str) -> Element {
    rsx! {
        span {
            class: "f-cover",
            if let Some(url) = cover {
                img { src: "{url}", alt: "" }
            } else {
                span { class: "f-cover__fallback", "{title}" }
            }
        }
    }
}

fn cover_url(model: ModelSignal, id: &str) -> Option<String> {
    let url = model.read().covers.get(id).cloned().unwrap_or_default();
    (!url.is_empty()).then_some(url)
}

fn by_title(items: &[CatalogItem], ids: Vec<String>) -> Vec<String> {
    let mut ids = ids;
    ids.sort_by(|left, right| {
        title_of(items, left)
            .to_ascii_lowercase()
            .cmp(&title_of(items, right).to_ascii_lowercase())
    });
    ids
}

fn kind_label(items: &[CatalogItem], id: &str) -> &'static str {
    let kind = items
        .iter()
        .find(|item| item.id == id)
        .map(|item| item.file_type.as_str());
    match kind {
        Some("epub") => "EPUB",
        Some("pdf") => "PDF",
        Some("mobi") => "MOBI",
        Some("cbr") => "CBR",
        Some("cbz") => "CBZ",
        Some("cb7") => "CB7",
        Some("cbt") => "CBT",
        Some("image-folder") => "FOLDER",
        _ => "BOOK",
    }
}

fn search_rows(items: &[CatalogItem]) -> Vec<SearchRow> {
    items
        .iter()
        .map(|item| SearchRow {
            id: item.id.clone(),
            label: item.label.clone(),
            title: item.title.clone(),
            enriched: item.enriched_fingerprint.is_some(),
        })
        .collect()
}
