//! Kavita beside the local shelf. This is not a chrome tab.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use dioxus::prelude::*;
use folio_formats::{render_pdf_png, sanitize_kavita_html};
use folio_platform::SecretStore;
use folio_reader::{
    cycle_zoom, image_fit_css, paragraph_css, reader_font, zoom_factor, Fit, SessionZoom, TextAlign,
};
use folio_storage::{
    kavita_downloads, servers, visible_kavita_chapters, visible_kavita_collections,
    visible_kavita_filters, visible_kavita_mirror, visible_kavita_reading_lists,
    visible_kavita_series, KavitaBookmarkRow, KavitaChapterRow, KavitaDownloadRow, KavitaHomeRow,
    KavitaLibraryRow, KavitaNamedRow, KavitaSeriesRow,
};

use crate::kavita::{
    bytes_data_url, cancel_downloads, chapter_directory, connection_failure, covers_to_fetch,
    directory_complete, download_root, fetch_and_publish, fetch_chapters, fetch_cover, fetch_page,
    list_page, push_pending, queue_chapter, read_key, record_mark, record_page, recover_downloads,
    refresh_status, run_downloads, stored_page, update_kavita, DownloadPolicy, ListPage, PageStop,
};
use crate::session::{app_files_dir, page_data_url, save_setting, sync_now, SessionError};

use super::{bookmark, database, pressed, set_paper, AppModel, ModelSignal};
use crate::chrome::theme_attr;

pub(super) struct KavitaView {
    pub(super) on: bool,
    pub(super) server_id: String,
    pub(super) panel: String,
    pub(super) series: Vec<KavitaSeriesRow>,
    pub(super) libraries: Vec<KavitaLibraryRow>,
    pub(super) chapters: Vec<KavitaChapterRow>,
    pub(super) named: Vec<KavitaNamedRow>,
    pub(super) home: Vec<KavitaHomeRow>,
    pub(super) bookmarks: Vec<KavitaBookmarkRow>,
    pub(super) downloads: Vec<KavitaDownloadRow>,
    pub(super) on_deck: Vec<i64>,
    pub(super) added: Vec<i64>,
    pub(super) updated: Vec<i64>,
    pub(super) wants: Vec<i64>,
    pub(super) detail: i64,
    pub(super) reading_chapters: bool,
    pub(super) format_filter: String,
    pub(super) sort: String,
    pub(super) library_filter: i64,
    pub(super) covers: BTreeMap<i64, String>,
    pub(super) invalid_cert: bool,
    pub(super) wifi_only: bool,
    pub(super) busy: bool,
    /// A sync was asked for while one ran; the running one makes one more pass when it ends.
    pub(super) resync: bool,
    pub(super) covering: bool,
    pub(super) page: usize,
}

impl KavitaView {
    pub(super) fn new() -> Self {
        Self {
            on: false,
            server_id: String::new(),
            panel: "home".to_owned(),
            series: Vec::new(),
            libraries: Vec::new(),
            chapters: Vec::new(),
            named: Vec::new(),
            home: Vec::new(),
            bookmarks: Vec::new(),
            downloads: Vec::new(),
            on_deck: Vec::new(),
            added: Vec::new(),
            updated: Vec::new(),
            wants: Vec::new(),
            detail: 0,
            reading_chapters: false,
            format_filter: String::new(),
            sort: "name".to_owned(),
            library_filter: 0,
            covers: BTreeMap::new(),
            invalid_cert: false,
            wifi_only: true,
            busy: false,
            resync: false,
            covering: false,
            page: 0,
        }
    }
}

pub(super) struct RemoteRead {
    pub(super) server_id: String,
    pub(super) series_id: i64,
    pub(super) volume_id: i64,
    pub(super) library_id: i64,
    pub(super) chapter_id: i64,
    pub(super) title: String,
    pub(super) kind: String,
    pub(super) page: i64,
    pub(super) page_count: i64,
    pub(super) body: String,
    pub(super) html: bool,
    pub(super) failed: bool,
    pub(super) pdf: Option<Vec<u8>>,
}

impl KavitaView {
    fn detail_title(&self) -> String {
        self.series
            .iter()
            .find(|row| row.series_id == self.detail)
            .map(|row| row.name.clone())
            .unwrap_or_default()
    }
}

pub(super) fn activate_kavita(mut model: ModelSignal) {
    let id = {
        let guard = model.read();
        if !guard.kavita.server_id.is_empty()
            && guard
                .servers
                .iter()
                .any(|server| server.id == guard.kavita.server_id && server.kind == "kavita")
        {
            guard.kavita.server_id.clone()
        } else {
            guard
                .servers
                .iter()
                .find(|server| server.kind == "kavita")
                .map(|server| server.id.clone())
                .unwrap_or_default()
        }
    };
    {
        let mut guard = model.write();
        guard.kavita.on = true;
        guard.kavita.server_id = id.clone();
        guard.kavita.panel = "home".to_owned();
    }
    if id.is_empty() {
        model.write().status = "Add a Kavita server on the Servers tab.".to_owned();
        return;
    }
    refresh_kavita(model);
    let needs_sync = {
        let guard = model.read();
        guard.kavita.series.is_empty() && !guard.kavita.busy && !guard.kavita.server_id.is_empty()
    };
    if needs_sync {
        sync_accounts(model);
    }
}

pub(super) fn refresh_kavita(mut model: ModelSignal) {
    let id = model.read().kavita.server_id.clone();
    if id.is_empty() {
        return;
    }
    let loaded = load_view(&id);
    match loaded {
        Ok(view) => {
            let mut guard = model.write();
            guard.kavita.series = view.series;
            guard.kavita.libraries = view.libraries;
            guard.kavita.home = view.home;
            guard.kavita.on_deck = view.on_deck;
            guard.kavita.added = view.added;
            guard.kavita.updated = view.updated;
            guard.kavita.wants = view.wants;
            guard.kavita.bookmarks = view.bookmarks;
            guard.kavita.downloads = view.downloads;
            guard.kavita.invalid_cert = view.invalid_cert;
            guard.kavita.wifi_only = view.wifi_only;
            let panel = guard.kavita.panel.clone();
            let detail = guard.kavita.detail;
            drop(guard);
            load_panel(model, &panel, detail);
        }
        Err(error) => model.write().status = error.to_string(),
    }
}

pub(super) fn sync_accounts(mut model: ModelSignal) {
    {
        let mut guard = model.write();
        if guard.kavita.busy {
            guard.kavita.resync = true;
            return;
        }
        guard.kavita.busy = true;
        if kavita_saved(&guard) {
            guard.status = "Contacting Kavita.".to_owned();
        }
    }
    std::thread::spawn(move || loop {
        let detail = match sync_accounts_now() {
            Ok(detail) => detail,
            Err(error) => error.to_string(),
        };
        let again = {
            let mut guard = model.write();
            let again = guard.kavita.resync;
            guard.kavita.resync = false;
            guard.kavita.busy = again;
            guard.status = if again && kavita_saved(&guard) {
                "Contacting Kavita.".to_owned()
            } else {
                detail
            };
            again
        };
        if model.read().kavita.on {
            refresh_kavita(model);
        }
        if !again {
            return;
        }
    });
}

fn kavita_saved(guard: &AppModel) -> bool {
    guard.servers.iter().any(|server| server.kind == "kavita") || !guard.kavita.server_id.is_empty()
}

pub(super) fn recover_kavita_downloads() -> Result<(), SessionError> {
    let db = database()?;
    recover_downloads(db, &download_root(&app_files_dir()))
}

pub(super) fn kavita_screen(model: ModelSignal) -> Element {
    let server_id = model.read().kavita.server_id.clone();
    if server_id.is_empty() {
        return rsx! {
            p { class: "f-empty", "Add a Kavita server on the Servers tab. Type its URL and Auth Key there." }
        };
    }
    let panel = model.read().kavita.panel.clone();
    let busy = model.read().kavita.busy;
    rsx! {
        nav {
            class: "f-chips",
            {panel_button(model, "home", "Home", &panel)}
            {panel_button(model, "on-deck", "On Deck", &panel)}
            {panel_button(model, "added", "Added", &panel)}
            {panel_button(model, "updated", "Updated", &panel)}
            {panel_button(model, "series", "Series", &panel)}
            {panel_button(model, "collections", "Collections", &panel)}
            {panel_button(model, "lists", "Lists", &panel)}
            {panel_button(model, "want", "Want to Read", &panel)}
            {panel_button(model, "filters", "Filters", &panel)}
            {panel_button(model, "bookmarks", "Bookmarks", &panel)}
            {panel_button(model, "downloads", "Downloads", &panel)}
            {panel_button(model, "settings", "Server", &panel)}
            button {
                class: "f-chip",
                disabled: busy,
                onclick: move |_| sync_accounts(model),
                if busy { "Refreshing" } else { "Refresh" }
            }
        }
        {panel_body(model, &panel)}
    }
}

pub(super) fn remote_reading(model: ModelSignal) -> Element {
    let view = {
        let guard = model.read();
        guard.remote.as_ref().map(|remote| RemotePaint {
            title: remote.title.clone(),
            body: remote.body.clone(),
            html: remote.html,
            failed: remote.failed,
            page: remote.page,
            page_count: remote.page_count,
            menu: guard.menu,
            hide_progress: guard.hide_progress,
            paper: guard.paper.clone(),
            font_px: guard.font_px,
            align: guard.align.clone(),
            indent: guard.indent,
            follow_font: guard.follow_font,
            fit: guard.fit.clone(),
            zoom: guard.zoom,
            mode: guard.reading_mode.clone(),
        })
    };
    let Some(view) = view else {
        return rsx! { p { "No chapter is open." } };
    };
    let font = reader_font(view.follow_font, None, "serif");
    let align = align_from(&view.align);
    let text = paragraph_css(align, view.indent);
    let image = image_style(&view.fit, view.zoom);
    let overflow = if view.mode == "scroll" {
        "auto"
    } else {
        "hidden"
    };
    let colors = paper_colors(&view.paper);
    let shown = view.page.saturating_add(1);
    let total = view.page_count.max(0);
    let percent = if total == 0 {
        0
    } else {
        usize::try_from(shown.saturating_mul(100).div_euclid(total)).unwrap_or(0)
    };
    rsx! {
        div {
            style: "position:fixed;inset:0;overflow:{overflow};{colors}font-family:{font};font-size:{view.font_px}px;{text}",
            if view.failed {
                p { style: "padding:28px;", "This page could not be loaded." }
            } else if view.html {
                div {
                    style: "padding:28px;pointer-events:none;",
                    dangerous_inner_html: "{view.body}",
                }
            } else if !view.body.is_empty() {
                img {
                    style: "{image}",
                    src: "{view.body}",
                    alt: "Page",
                }
            } else {
                p { style: "padding:28px;", "Opening" }
            }
            {zones(model)}
            if view.menu {
                {reader_menu(model, &view.title, &format!("{shown} / {total}"), true)}
            } else if percent > 0 && !view.hide_progress {
                div {
                    style: "position:absolute;left:0;right:0;bottom:0;height:4px;z-index:2;pointer-events:none;",
                    div { style: "height:100%;width:{percent}%;background:currentColor;" }
                }
            }
        }
    }
}

pub(super) fn close_remote(mut model: ModelSignal) {
    let progress = {
        let guard = model.read();
        guard.remote.as_ref().map(|remote| {
            (
                remote.server_id.clone(),
                remote.series_id,
                remote.volume_id,
                remote.library_id,
                remote.chapter_id,
                remote.page,
                remote.page_count,
            )
        })
    };
    {
        let mut guard = model.write();
        guard.remote = None;
        guard.menu = false;
        guard.zoom = 1;
    }
    if let Some((server_id, series_id, volume_id, library_id, chapter_id, page, page_count)) =
        progress
    {
        if let Ok(db) = database() {
            if let Err(error) = record_page(
                db,
                &server_id,
                PageStop {
                    series_id,
                    volume_id,
                    library_id,
                    chapter_id,
                    page,
                    page_count,
                },
            ) {
                model.write().status = error.to_string();
            }
        }
    }
}

pub(super) fn turn_remote(model: ModelSignal, delta: i64) {
    let next = {
        let guard = model.read();
        let Some(remote) = guard.remote.as_ref() else {
            return;
        };
        let last = remote.page_count.saturating_sub(1).max(0);
        let page = remote.page.saturating_add(delta).clamp(0, last);
        if page == remote.page {
            return;
        }
        page
    };
    show_page(model, next);
}

pub(super) fn cycle_fit_pref(mut model: ModelSignal) {
    let next = match model.read().fit.as_str() {
        "screen" => "width",
        "width" => "height",
        "height" => "original",
        "original" => "smart",
        _ => "screen",
    };
    model.write().fit = next.to_owned();
    store_pref(model, "reader.fit", next);
}

pub(super) fn cycle_zoom_pref(mut model: ModelSignal) {
    let next = zoom_factor(cycle_zoom(zoom_from(model.read().zoom)));
    model.write().zoom = next;
}

pub(super) fn cycle_align_pref(mut model: ModelSignal) {
    let next = match model.read().align.as_str() {
        "start" => "center",
        "center" => "justify",
        _ => "start",
    };
    model.write().align = next.to_owned();
    store_pref(model, "reader.align", next);
}

pub(super) fn toggle_indent_pref(mut model: ModelSignal) {
    let next = !model.read().indent;
    model.write().indent = next;
    store_pref(model, "reader.indent", on_off(next));
}

pub(super) fn toggle_progress_pref(mut model: ModelSignal) {
    let next = !model.read().hide_progress;
    model.write().hide_progress = next;
    store_pref(model, "reader.hide_progress", on_off(next));
}

pub(super) fn toggle_font_pref(mut model: ModelSignal) {
    let next = !model.read().follow_font;
    model.write().follow_font = next;
    store_pref(model, "reader.follow_font", on_off(next));
}

pub(super) fn toggle_mode_pref(mut model: ModelSignal) {
    let next = if model.read().reading_mode == "scroll" {
        "paginated"
    } else {
        "scroll"
    };
    model.write().reading_mode = next.to_owned();
    store_pref(model, "reader.mode", next);
}

pub(super) fn mark_remote(mut model: ModelSignal, scope: &str, read: bool) {
    let scope = scope.to_owned();
    let marked = {
        let guard = model.read();
        guard.remote.as_ref().map(|remote| {
            (
                remote.server_id.clone(),
                remote.series_id,
                remote.volume_id,
                remote.chapter_id,
            )
        })
    };
    let Some((server_id, series_id, volume_id, chapter_id)) = marked else {
        return;
    };
    let scope_id = match scope.as_str() {
        "volume" => volume_id,
        "series" => series_id,
        _ => chapter_id,
    };
    std::thread::spawn(move || {
        let result = (|| {
            let db = database()?;
            record_mark(db, &server_id, &scope, scope_id, series_id, volume_id, read)?;
            let (base, key, allow) = account(&server_id)?;
            push_pending(db, &server_id, &base, &key, allow)
        })();
        match result {
            Ok(true) => model.write().status = "Progress was delivered.".to_owned(),
            Ok(false) => {
                model.write().status =
                    "The mark stays on this device until the server answers.".to_owned()
            }
            Err(error) => model.write().status = error.to_string(),
        }
    });
}

pub(super) fn local_paragraph_css(align: &str, indent: bool) -> &'static str {
    paragraph_css(align_from(align), indent)
}

pub(super) fn local_image_style(fit: &str, zoom: u8) -> String {
    image_style(fit, zoom)
}

pub(super) fn local_font(follow: bool, fallback: &str) -> &str {
    reader_font(follow, None, fallback)
}

fn panel_button(
    model: ModelSignal,
    id: &'static str,
    label: &'static str,
    current: &str,
) -> Element {
    rsx! {
        button {
            key: "{id}",
            class: "f-chip",
            aria_pressed: pressed(current == id),
            onclick: move |_| open_panel(model, id),
            "{label}"
        }
    }
}

fn panel_body(model: ModelSignal, panel: &str) -> Element {
    // The ids are copied out first: `series_panel` reads the model again, and a second read
    // while this one is held deadlocks once a background thread is waiting to write.
    let shelf = {
        let guard = model.read();
        shelf_page(&guard, panel)
    };
    if let Some(shelf) = shelf {
        return series_panel(model, shelf);
    }
    match panel {
        "settings" => settings_panel(model),
        "downloads" => downloads_panel(model),
        "detail" => detail_panel(model),
        "collections" | "lists" | "filters" => named_panel(model, panel),
        "bookmarks" => bookmarks_panel(model),
        _ => home_panel(model),
    }
}

fn home_panel(model: ModelSignal) -> Element {
    // Every read happens here, before `rsx!`: `shelf_block` reads the model again through
    // `series_card`, and a second read while a guard is held deadlocks once a background
    // thread is waiting to write.
    let (streams, libraries, on_deck, added, updated) = {
        let guard = model.read();
        let streams = guard
            .kavita
            .home
            .iter()
            .filter(|row| row.visible)
            .map(|row| row.name.clone())
            .collect::<Vec<_>>();
        (
            streams,
            guard.kavita.libraries.clone(),
            guard.kavita.on_deck.clone(),
            guard.kavita.added.clone(),
            guard.kavita.updated.clone(),
        )
    };
    rsx! {
        if !streams.is_empty() {
            h2 { class: "f-h2", "Dashboard" }
            ul {
                class: "f-list",
                for name in streams {
                    li { key: "{name}", class: "f-list__item", "{name}" }
                }
            }
        }
        if !libraries.is_empty() {
            div {
                class: "f-chips",
                for library in libraries {
                    button {
                        key: "{library.library_id}",
                        class: "f-chip",
                        onclick: move |_| open_library(model, library.library_id),
                        "{library.name}"
                    }
                }
            }
        }
        {shelf_block(model, "On Deck", on_deck)}
        {shelf_block(model, "Recently added", added)}
        {shelf_block(model, "Recently updated", updated)}
    }
}

fn shelf_block(model: ModelSignal, title: &str, ids: Vec<i64>) -> Element {
    if ids.is_empty() {
        return rsx! {};
    }
    let title = title.to_owned();
    rsx! {
        section {
            h2 { class: "f-h2", "{title}" }
            {series_cards(model, ids)}
        }
    }
}

fn series_panel(model: ModelSignal, shelf: ShelfPage) -> Element {
    let shown = shelf.shown().to_vec();
    let page = shelf.page;
    rsx! {
        {filter_row(model)}
        if shown.is_empty() {
            p { class: "f-empty", "No series in this list." }
        } else {
            {pager(model, page)}
            {series_cards(model, shown)}
            {pager(model, page)}
        }
    }
}

fn pager(model: ModelSignal, page: ListPage) -> Element {
    if page.pages < 2 {
        return rsx! {};
    }
    let label = format!(
        "{}–{} of {}",
        page.start.saturating_add(1),
        page.end,
        page.total
    );
    let last = page.index.saturating_add(1) >= page.pages;
    rsx! {
        div {
            class: "f-chips",
            button {
                class: "f-chip",
                disabled: page.index == 0,
                onclick: move |_| turn_series_page(model, -1),
                "Previous"
            }
            p { class: "f-meta", "{label}" }
            button {
                class: "f-chip",
                disabled: last,
                onclick: move |_| turn_series_page(model, 1),
                "Next"
            }
        }
    }
}

fn turn_series_page(mut model: ModelSignal, delta: i64) {
    {
        let mut guard = model.write();
        let current = i64::try_from(guard.kavita.page).unwrap_or(i64::MAX);
        guard.kavita.page = usize::try_from(current.saturating_add(delta)).unwrap_or(0);
    }
    fetch_covers(model);
    // Next may sit under the grid; the new page should start at its first card.
    dioxus::document::eval("window.scrollTo(0, 0);");
}

fn series_cards(model: ModelSignal, ids: Vec<i64>) -> Element {
    let cards = ids
        .into_iter()
        .filter_map(|id| series_card(model, id))
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

fn series_card(model: ModelSignal, id: i64) -> Option<Element> {
    let row = model
        .read()
        .kavita
        .series
        .iter()
        .find(|row| row.series_id == id)
        .cloned();
    let row = row?;
    let cover = model
        .read()
        .kavita
        .covers
        .get(&id)
        .cloned()
        .filter(|url| !url.is_empty());
    let name = row.name.clone();
    rsx! {
        button {
            key: "{id}",
            class: "f-book",
            onclick: move |_| open_series(model, id),
            span {
                class: "f-cover",
                if let Some(url) = cover {
                    img { src: "{url}", alt: "" }
                } else {
                    span { class: "f-cover__fallback", "{name}" }
                }
            }
            span { class: "f-book__title", "{name}" }
        }
    }
    .into()
}

fn detail_panel(model: ModelSignal) -> Element {
    let title = model.read().kavita.detail_title();
    let chapters = model.read().kavita.chapters.clone();
    let empty = if model.read().kavita.reading_chapters {
        "Reading chapters from Kavita."
    } else {
        "No chapters."
    };
    rsx! {
        div {
            class: "f-detail-head",
            button {
                class: "f-btn",
                onclick: move |_| open_panel(model, "series"),
                "Back"
            }
            h2 { class: "f-h2 f-h2--flush", "{title}" }
        }
        if chapters.is_empty() {
            p { class: "f-empty", "{empty}" }
        } else {
            ul {
                class: "f-list",
                for chapter in chapters {
                    li {
                        key: "{chapter.chapter_id}",
                        class: "f-list__item f-list__item--actions",
                        span { class: "f-list__text", "{chapter.name}" }
                        span {
                            class: "f-list__buttons",
                            button {
                                class: "f-btn",
                                onclick: move |_| open_chapter(model, chapter.chapter_id),
                                "Read"
                            }
                            button {
                                class: "f-btn",
                                onclick: move |_| queue_and_run(model, chapter.chapter_id),
                                "Download"
                            }
                        }
                    }
                }
            }
        }
    }
}

fn named_panel(model: ModelSignal, panel: &str) -> Element {
    let rows = model.read().kavita.named.clone();
    let empty = match panel {
        "collections" => "No collections.",
        "lists" => "No reading lists.",
        _ => "No smart filters.",
    };
    rsx! {
        if rows.is_empty() {
            p { class: "f-empty", "{empty}" }
        } else {
            ul {
                class: "f-list",
                for row in rows {
                    li { key: "{row.id}", class: "f-list__item", "{row.name}" }
                }
            }
        }
    }
}

fn bookmarks_panel(model: ModelSignal) -> Element {
    let rows = model.read().kavita.bookmarks.clone();
    rsx! {
        if rows.is_empty() {
            p { class: "f-empty", "No bookmarks." }
        } else {
            div {
                class: "f-stack",
                for row in rows {
                    button {
                        key: "{row.bookmark_id}",
                        class: "f-btn f-btn--block",
                        onclick: move |_| open_chapter_page(model, row.chapter_id, row.page),
                        "Page {row.page} in chapter {row.chapter_id}"
                    }
                }
            }
        }
    }
}

fn downloads_panel(model: ModelSignal) -> Element {
    let rows = model.read().kavita.downloads.clone();
    let wifi = model.read().kavita.wifi_only;
    let wifi_label = if wifi {
        "Wi-Fi only: on"
    } else {
        "Wi-Fi only: off"
    };
    rsx! {
        div {
            class: "f-actions",
            button { class: "f-btn", onclick: move |_| toggle_wifi(model), "{wifi_label}" }
            button { class: "f-btn", onclick: move |_| run_queue(model), "Start" }
            button { class: "f-btn", onclick: move |_| stop_queue(model), "Stop" }
        }
        if rows.is_empty() {
            p { class: "f-empty", "No downloads." }
        } else {
            ul {
                class: "f-list",
                for row in rows {
                    li {
                        key: "{row.id}",
                        class: "f-list__item f-list__item--split",
                        span { class: "f-list__text", "{row.id}" }
                        span { class: "f-meta", "{row.status}" }
                    }
                }
            }
        }
    }
}

fn settings_panel(mut model: ModelSignal) -> Element {
    let name = model.read().server_name.clone();
    let url = model.read().server_url.clone();
    let secret = model.read().server_secret.clone();
    let invalid = model.read().kavita.invalid_cert;
    let invalid_label = if invalid {
        "Invalid certificates: on"
    } else {
        "Invalid certificates: off"
    };
    rsx! {
        section {
            class: "f-card",
            div {
                class: "f-field",
                label { class: "f-label", r#for: "kavita-name", "Name" }
                input {
                    id: "kavita-name",
                    class: "f-input",
                    value: "{name}",
                    oninput: move |event| model.write().server_name = event.value(),
                }
            }
            div {
                class: "f-field",
                label { class: "f-label", r#for: "kavita-url", "URL" }
                input {
                    id: "kavita-url",
                    class: "f-input",
                    value: "{url}",
                    oninput: move |event| model.write().server_url = event.value(),
                }
            }
            div {
                class: "f-field",
                label { class: "f-label", r#for: "kavita-key", "Auth Key" }
                input {
                    id: "kavita-key",
                    class: "f-input",
                    value: "{secret}",
                    placeholder: "Leave blank to keep the saved key",
                    oninput: move |event| model.write().server_secret = event.value(),
                }
            }
            div {
                class: "f-actions",
                button { class: "f-btn", onclick: move |_| toggle_invalid(model), "{invalid_label}" }
            }
            div {
                class: "f-actions",
                button {
                    class: "f-btn f-btn--action",
                    onclick: move |_| test_panel(model),
                    "Test connection"
                }
                button {
                    class: "f-btn f-btn--primary f-btn--action",
                    onclick: move |_| save_server(model),
                    "Save"
                }
            }
        }
    }
}

fn filter_row(mut model: ModelSignal) -> Element {
    let sort = model.read().kavita.sort.clone();
    let format_filter = model.read().kavita.format_filter.clone();
    let library_label = library_label(&model);
    rsx! {
        div {
            class: "f-chips",
            button {
                class: "f-chip",
                onclick: move |_| {
                    {
                        let mut guard = model.write();
                        guard.kavita.sort = if sort == "name" { "id".to_owned() } else { "name".to_owned() };
                        guard.kavita.page = 0;
                    }
                    fetch_covers(model);
                },
                "Sort"
            }
            button {
                class: "f-chip",
                onclick: move |_| cycle_library(model),
                "{library_label}"
            }
            button {
                class: "f-chip",
                onclick: move |_| cycle_format(model),
                if format_filter.is_empty() { "All formats" } else { "{format_filter}" }
            }
        }
    }
}

fn zones(model: ModelSignal) -> Element {
    rsx! {
        div {
            style: "position:absolute;inset:0;display:flex;z-index:1;",
            button {
                style: ZONE,
                onclick: move |_| super::zone_tapped(model, folio_reader::Side::Left),
                " "
            }
            button {
                style: ZONE,
                onclick: move |_| super::zone_tapped(model, folio_reader::Side::Center),
                " "
            }
            button {
                style: ZONE,
                onclick: move |_| super::zone_tapped(model, folio_reader::Side::Right),
                " "
            }
        }
    }
}

pub(super) fn reader_menu(
    model: ModelSignal,
    title: &str,
    progress: &str,
    remote: bool,
) -> Element {
    let fit = model.read().fit.clone();
    let mode = model.read().reading_mode.clone();
    let align = model.read().align.clone();
    let zoom = model.read().zoom;
    let paper = model.read().paper.clone();
    let theme = theme_attr(&paper);
    rsx! {
        div {
            class: "f-menu f-menu--top",
            "data-theme": "{theme}",
            p { class: "f-menu__title", "{title}" }
            button { class: "f-btn", onclick: move |_| super::close_book(model), "Close" }
        }
        div {
            class: "f-menu f-menu--bottom",
            "data-theme": "{theme}",
            p { class: "f-menu__progress", "{progress}" }
            div {
                class: "f-menu__group",
                p { class: "f-menu__label", "Text" }
                div {
                    class: "f-menu__buttons",
                    button { class: "f-btn", onclick: move |_| super::change_font(model, false), "A-" }
                    button { class: "f-btn", onclick: move |_| super::change_font(model, true), "A+" }
                    button { class: "f-btn", onclick: move |_| cycle_align_pref(model), "{align}" }
                    button { class: "f-btn", onclick: move |_| toggle_indent_pref(model), "Indent" }
                    button { class: "f-btn", onclick: move |_| toggle_font_pref(model), "Server font" }
                }
            }
            div {
                class: "f-menu__group",
                p { class: "f-menu__label", "Page" }
                div {
                    class: "f-menu__buttons",
                    button { class: "f-btn", onclick: move |_| cycle_fit_pref(model), "Fit {fit}" }
                    button { class: "f-btn", onclick: move |_| cycle_zoom_pref(model), "Zoom {zoom}" }
                    button { class: "f-btn", onclick: move |_| toggle_mode_pref(model), "{mode}" }
                    button { class: "f-btn", onclick: move |_| toggle_progress_pref(model), "Progress bar" }
                }
            }
            div {
                class: "f-menu__group",
                p { class: "f-menu__label", "Paper" }
                div {
                    class: "f-seg",
                    for (value, label) in PAPERS {
                        button {
                            key: "{value}",
                            class: "f-seg__item",
                            aria_pressed: pressed(paper == value),
                            onclick: move |_| set_paper(model, value.to_owned()),
                            "{label}"
                        }
                    }
                }
            }
            if remote {
                div {
                    class: "f-menu__group",
                    p { class: "f-menu__label", "Marks" }
                    div {
                        class: "f-menu__buttons",
                        button { class: "f-btn", onclick: move |_| mark_remote(model, "chapter", true), "Chapter read" }
                        button { class: "f-btn", onclick: move |_| mark_remote(model, "chapter", false), "Chapter unread" }
                        button { class: "f-btn", onclick: move |_| mark_remote(model, "volume", true), "Volume read" }
                        button { class: "f-btn", onclick: move |_| mark_remote(model, "volume", false), "Volume unread" }
                        button { class: "f-btn", onclick: move |_| mark_remote(model, "series", true), "Series read" }
                        button { class: "f-btn", onclick: move |_| mark_remote(model, "series", false), "Series unread" }
                    }
                }
            } else {
                div {
                    class: "f-menu__group",
                    div {
                        class: "f-menu__buttons",
                        button { class: "f-btn", onclick: move |_| bookmark(model), "Bookmark" }
                    }
                }
            }
        }
    }
}

fn open_panel(mut model: ModelSignal, panel: &'static str) {
    if panel != "downloads" && model.read().kavita.panel == "downloads" {
        stop_queue(model);
    }
    {
        let mut guard = model.write();
        guard.kavita.panel = panel.to_owned();
        guard.kavita.page = 0;
        if panel == "settings" {
            let id = guard.kavita.server_id.clone();
            let saved = guard
                .servers
                .iter()
                .find(|server| server.id == id)
                .map(|server| (server.display_name.clone(), server.url.clone()));
            if let Some((name, url)) = saved {
                guard.server_name = name;
                guard.server_url = url;
                guard.server_secret.clear();
            }
        }
    }
    let detail = model.read().kavita.detail;
    load_panel(model, panel, detail);
}

fn open_library(mut model: ModelSignal, library_id: i64) {
    model.write().kavita.library_filter = library_id;
    open_panel(model, "series");
}

fn open_series(mut model: ModelSignal, series_id: i64) {
    model.write().kavita.detail = series_id;
    model.write().kavita.panel = "detail".to_owned();
    model.write().kavita.reading_chapters = true;
    load_panel(model, "detail", series_id);
    let server_id = model.read().kavita.server_id.clone();
    std::thread::spawn(move || {
        let outcome = (|| {
            let (base, key, allow) = account(&server_id)?;
            let store = SecretStore::open(&app_files_dir())?;
            fetch_chapters(
                database()?,
                &store,
                &server_id,
                &base,
                &key,
                allow,
                series_id,
            )
        })();
        let still_open = {
            let guard = model.read();
            guard.kavita.detail == series_id && guard.kavita.panel == "detail"
        };
        if still_open {
            model.write().kavita.reading_chapters = false;
        }
        match outcome {
            Ok(()) if still_open => load_panel(model, "detail", series_id),
            Ok(()) => {}
            Err(error) => model.write().status = connection_failure(&error),
        }
    });
}

fn open_chapter(model: ModelSignal, chapter_id: i64) {
    open_chapter_page(model, chapter_id, -1);
}

fn open_chapter_page(mut model: ModelSignal, chapter_id: i64, saved_page: i64) {
    let opened = {
        let guard = model.read();
        let chapter = guard
            .kavita
            .chapters
            .iter()
            .find(|row| row.chapter_id == chapter_id)
            .cloned();
        let server_id = guard.kavita.server_id.clone();
        let series = chapter.as_ref().and_then(|chapter| {
            guard
                .kavita
                .series
                .iter()
                .find(|row| row.series_id == chapter.series_id)
                .cloned()
        });
        (server_id, chapter, series)
    };
    let (server_id, chapter, series) = opened;
    let Some(chapter) = chapter else {
        model.write().status = "That chapter is not in the mirror.".to_owned();
        return;
    };
    let kind = series
        .as_ref()
        .map(|row| row.format.clone())
        .unwrap_or_else(|| "image".to_owned());
    let library_id = series.as_ref().map(|row| row.library_id).unwrap_or(0);
    let page = if saved_page >= 0 { saved_page } else { 0 };
    {
        let mut guard = model.write();
        guard.remote_epoch = guard.remote_epoch.saturating_add(1);
        guard.menu = false;
        guard.zoom = 1;
        guard.remote = Some(RemoteRead {
            server_id: server_id.clone(),
            series_id: chapter.series_id,
            volume_id: chapter.volume_id,
            library_id,
            chapter_id,
            title: chapter.name.clone(),
            kind: kind.clone(),
            page,
            page_count: chapter.pages.max(1),
            body: String::new(),
            html: kind == "epub",
            failed: false,
            pdf: None,
        });
        guard.status = "Opening".to_owned();
    }
    show_page(model, page);
}

fn show_page(mut model: ModelSignal, page: i64) {
    let request = {
        let mut guard = model.write();
        guard.remote_epoch = guard.remote_epoch.saturating_add(1);
        let epoch = guard.remote_epoch;
        let Some(remote) = guard.remote.as_mut() else {
            return;
        };
        remote.page = page;
        remote.failed = false;
        (
            epoch,
            remote.server_id.clone(),
            remote.kind.clone(),
            remote.chapter_id,
            remote.page_count,
            page,
        )
    };
    let (epoch, server_id, kind, chapter_id, page_count, page) = request;
    std::thread::spawn(move || {
        let result = load_page_body(&server_id, &kind, chapter_id, page, page_count);
        let mut guard = model.write();
        if guard.remote_epoch != epoch {
            return;
        }
        let Some(remote) = guard.remote.as_mut() else {
            return;
        };
        match result {
            Ok(loaded) => {
                remote.body = loaded.body;
                remote.html = loaded.html;
                remote.pdf = loaded.pdf;
                remote.failed = false;
                guard.status.clear();
            }
            Err(error) => {
                remote.failed = true;
                guard.status = error.to_string();
            }
        }
        let progress = guard.remote.as_ref().map(|remote| {
            (
                remote.server_id.clone(),
                remote.series_id,
                remote.volume_id,
                remote.library_id,
                remote.chapter_id,
                remote.page,
                remote.page_count,
            )
        });
        drop(guard);
        if let Some((server_id, series_id, volume_id, library_id, chapter_id, page, page_count)) =
            progress
        {
            if let Ok(db) = database() {
                if let Err(error) = record_page(
                    db,
                    &server_id,
                    PageStop {
                        series_id,
                        volume_id,
                        library_id,
                        chapter_id,
                        page,
                        page_count,
                    },
                ) {
                    model.write().status = error.to_string();
                }
            }
        }
    });
}

struct LoadedPage {
    body: String,
    html: bool,
    pdf: Option<Vec<u8>>,
}

fn load_page_body(
    server_id: &str,
    kind: &str,
    chapter_id: i64,
    page: i64,
    page_count: i64,
) -> Result<LoadedPage, SessionError> {
    let directory = chapter_directory(&download_root(&app_files_dir()), server_id, chapter_id);
    let downloaded = directory_complete(&directory, page_count);
    if downloaded {
        return page_from_disk(&directory, kind, page);
    }
    let (base, key, allow) = account(server_id)?;
    let bytes = fetch_page(&base, &key, kind, chapter_id, page, allow)?;
    paint_bytes(kind, page, bytes)
}

fn page_from_disk(directory: &Path, kind: &str, page: i64) -> Result<LoadedPage, SessionError> {
    let bytes = stored_page(directory, kind, page).ok_or(SessionError::Server)?;
    paint_bytes(kind, page, bytes)
}

fn paint_bytes(kind: &str, page: i64, bytes: Vec<u8>) -> Result<LoadedPage, SessionError> {
    if kind == "epub" {
        let html = String::from_utf8(bytes).map_err(|_| SessionError::Server)?;
        Ok(LoadedPage {
            body: sanitize_kavita_html(&html),
            html: true,
            pdf: None,
        })
    } else if kind == "pdf" {
        let index = usize::try_from(page).map_err(|_| SessionError::Number)?;
        let png = render_pdf_png(&bytes, index)?;
        Ok(LoadedPage {
            body: page_data_url("page.png", &png),
            html: false,
            pdf: Some(bytes),
        })
    } else {
        Ok(LoadedPage {
            body: page_data_url(image_name(&bytes), &bytes),
            html: false,
            pdf: None,
        })
    }
}

fn queue_and_run(mut model: ModelSignal, chapter_id: i64) {
    let queued = {
        let guard = model.read();
        let server_id = guard.kavita.server_id.clone();
        let kind = guard
            .kavita
            .chapters
            .iter()
            .find(|row| row.chapter_id == chapter_id)
            .and_then(|chapter| {
                guard
                    .kavita
                    .series
                    .iter()
                    .find(|series| series.series_id == chapter.series_id)
                    .map(|series| series.format.clone())
            })
            .unwrap_or_else(|| "image".to_owned());
        let pages = guard
            .kavita
            .chapters
            .iter()
            .find(|row| row.chapter_id == chapter_id)
            .map(|row| row.pages)
            .unwrap_or(0);
        (server_id, kind, pages)
    };
    std::thread::spawn(move || {
        let (server_id, kind, pages) = queued;
        let outcome = (|| {
            let (base, key, allow) = account(&server_id)?;
            let unmetered = !model.read().kavita.wifi_only || link_unmetered();
            let db = database()?;
            queue_chapter(db, &server_id, &base, chapter_id, &kind)?;
            let root = download_root(&app_files_dir());
            let mut needle = String::from("kavita-");
            needle.push_str(&server_id);
            needle.push('-');
            needle.push_str(&chapter_id.to_string());
            run_downloads(
                db,
                &root,
                &base,
                &key,
                |id| {
                    if id == needle {
                        Some((kind.clone(), pages))
                    } else {
                        None
                    }
                },
                DownloadPolicy {
                    allow_invalid_certs: allow,
                    unmetered,
                    foreground: true,
                },
            )
        })();
        match outcome {
            Ok(()) => model.write().status = "Download finished.".to_owned(),
            Err(error) => model.write().status = error.to_string(),
        }
        refresh_kavita(model);
    });
}

fn run_queue(mut model: ModelSignal) {
    model.write().kavita.panel = "downloads".to_owned();
    let server_id = model.read().kavita.server_id.clone();
    std::thread::spawn(move || {
        let outcome = (|| {
            let (base, key, allow) = account(&server_id)?;
            let unmetered = !model.read().kavita.wifi_only || link_unmetered();
            let db = database()?;
            let chapters = visible_kavita_chapters_for(db, &server_id)?;
            run_downloads(
                db,
                &download_root(&app_files_dir()),
                &base,
                &key,
                |id| chapter_kind(id, &chapters),
                DownloadPolicy {
                    allow_invalid_certs: allow,
                    unmetered,
                    foreground: true,
                },
            )
        })();
        if let Err(error) = outcome {
            model.write().status = error.to_string();
        }
        refresh_kavita(model);
    });
}

fn stop_queue(mut model: ModelSignal) {
    if let Ok(db) = database() {
        if let Err(error) = cancel_downloads(db, &download_root(&app_files_dir())) {
            model.write().status = error.to_string();
            return;
        }
    }
    refresh_kavita(model);
}

fn test_panel(mut model: ModelSignal) {
    let (id, url, secret, saved_url) = {
        let guard = model.read();
        let id = guard.kavita.server_id.clone();
        let saved_url = guard
            .servers
            .iter()
            .find(|server| server.id == id)
            .map(|server| server.url.clone());
        (
            id,
            guard.server_url.clone(),
            guard.server_secret.clone(),
            saved_url,
        )
    };
    if !(url.is_empty() || url.starts_with("http://") || url.starts_with("https://")) {
        model.write().status = "Type a URL that starts with http:// or https://.".to_owned();
        return;
    }
    let target = if url.starts_with("http://") || url.starts_with("https://") {
        url
    } else if let Some(saved_url) = saved_url {
        saved_url
    } else {
        model.write().status = "Type a URL that starts with http:// or https://.".to_owned();
        return;
    };
    let key = if secret.is_empty() {
        None
    } else {
        Some(secret)
    };
    super::test_address(model, target, key, id);
}

fn save_server(mut model: ModelSignal) {
    let (id, name, url, secret, invalid) = {
        let guard = model.read();
        (
            guard.kavita.server_id.clone(),
            guard.server_name.clone(),
            guard.server_url.clone(),
            guard.server_secret.clone(),
            guard.kavita.invalid_cert,
        )
    };
    let saved = (|| {
        let db = database()?;
        let store = SecretStore::open(&app_files_dir())?;
        update_kavita(db, &store, &id, &name, &url, &secret)?;
        save_setting(db, &invalid_key(&id), on_off(invalid))?;
        Ok::<(), SessionError>(())
    })();
    match saved {
        Ok(()) => {
            model.write().server_secret.clear();
            model.write().status = "Server saved.".to_owned();
            super::reload(model);
            refresh_kavita(model);
        }
        Err(error) => model.write().status = error.to_string(),
    }
}

fn toggle_invalid(mut model: ModelSignal) {
    let next = !model.read().kavita.invalid_cert;
    model.write().kavita.invalid_cert = next;
}

fn toggle_wifi(mut model: ModelSignal) {
    let next = !model.read().kavita.wifi_only;
    model.write().kavita.wifi_only = next;
    if let Ok(db) = database() {
        if let Err(error) = save_setting(db, "kavita.wifi_only", on_off(next)) {
            model.write().status = error.to_string();
        }
    }
}

fn cycle_library(mut model: ModelSignal) {
    let next = {
        let guard = model.read();
        let ids = guard
            .kavita
            .libraries
            .iter()
            .map(|row| row.library_id)
            .collect::<Vec<_>>();
        let current = guard.kavita.library_filter;
        if ids.is_empty() {
            0
        } else if current == 0 {
            ids[0]
        } else {
            match ids.iter().position(|id| *id == current) {
                Some(index) if index.saturating_add(1) < ids.len() => ids[index.saturating_add(1)],
                _ => 0,
            }
        }
    };
    {
        let mut guard = model.write();
        guard.kavita.library_filter = next;
        guard.kavita.page = 0;
    }
    fetch_covers(model);
}

fn library_label(model: &ModelSignal) -> String {
    let guard = model.read();
    let id = guard.kavita.library_filter;
    if id == 0 {
        return "All libraries".to_owned();
    }
    guard
        .kavita
        .libraries
        .iter()
        .find(|row| row.library_id == id)
        .map(|row| row.name.clone())
        .unwrap_or_else(|| "All libraries".to_owned())
}

fn cycle_format(mut model: ModelSignal) {
    let next = match model.read().kavita.format_filter.as_str() {
        "" => "epub",
        "epub" => "pdf",
        "pdf" => "image",
        "image" => "archive",
        _ => "",
    };
    {
        let mut guard = model.write();
        guard.kavita.format_filter = next.to_owned();
        guard.kavita.page = 0;
    }
    fetch_covers(model);
}

/// Ids a shelf panel lists before filtering, or `None` for a panel that is not a shelf.
fn shelf_ids(view: &KavitaView, panel: &str) -> Option<Vec<i64>> {
    match panel {
        "series" => Some(view.series.iter().map(|row| row.series_id).collect()),
        "on-deck" => Some(view.on_deck.clone()),
        "added" => Some(view.added.clone()),
        "updated" => Some(view.updated.clone()),
        "want" => Some(view.wants.clone()),
        _ => None,
    }
}

const SERIES_PAGE: usize = 40;

/// A shelf panel's ids after the filters and sort, and the page of them on screen.
struct ShelfPage {
    ids: Vec<i64>,
    page: ListPage,
}

impl ShelfPage {
    fn shown(&self) -> &[i64] {
        &self.ids[self.page.start..self.page.end]
    }
}

fn shelf_page(guard: &AppModel, panel: &str) -> Option<ShelfPage> {
    let ids = sorted_ids(guard, shelf_ids(&guard.kavita, panel)?);
    let page = list_page(ids.len(), guard.kavita.page, SERIES_PAGE);
    Some(ShelfPage { ids, page })
}

/// `ids` after the search query, the format and library filters and the sort, in screen order.
fn sorted_ids(guard: &AppModel, ids: Vec<i64>) -> Vec<i64> {
    let query = guard.query.to_ascii_lowercase();
    let format_filter = guard.kavita.format_filter.as_str();
    let library_filter = guard.kavita.library_filter;
    let listed = ids.into_iter().collect::<BTreeSet<_>>();
    let mut rows = guard
        .kavita
        .series
        .iter()
        .filter(|row| listed.contains(&row.series_id))
        .filter(|row| format_filter.is_empty() || row.format == format_filter)
        .filter(|row| library_filter == 0 || row.library_id == library_filter)
        .filter(|row| query.is_empty() || row.name.to_ascii_lowercase().contains(&query))
        .collect::<Vec<_>>();
    if guard.kavita.sort == "id" {
        rows.sort_by_key(|row| row.series_id);
    } else {
        rows.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        });
    }
    rows.into_iter().map(|row| row.series_id).collect()
}

fn load_panel(mut model: ModelSignal, panel: &str, detail: i64) {
    let server_id = model.read().kavita.server_id.clone();
    let loaded = (|| {
        let db = database()?;
        let named = match panel {
            "collections" => visible_kavita_collections(db, &server_id)?,
            "lists" => visible_kavita_reading_lists(db, &server_id)?,
            "filters" => visible_kavita_filters(db, &server_id)?,
            _ => Vec::new(),
        };
        let chapters = if panel == "detail" {
            visible_kavita_chapters(db, &server_id, detail)?
        } else {
            Vec::new()
        };
        Ok::<_, SessionError>((named, chapters))
    })();
    match loaded {
        Ok((named, chapters)) => {
            let mut guard = model.write();
            guard.kavita.named = named;
            guard.kavita.chapters = chapters;
        }
        Err(error) => model.write().status = error.to_string(),
    }
    fetch_covers(model);
}

struct ViewLists {
    series: Vec<KavitaSeriesRow>,
    libraries: Vec<KavitaLibraryRow>,
    home: Vec<KavitaHomeRow>,
    on_deck: Vec<i64>,
    added: Vec<i64>,
    updated: Vec<i64>,
    wants: Vec<i64>,
    bookmarks: Vec<KavitaBookmarkRow>,
    downloads: Vec<KavitaDownloadRow>,
    invalid_cert: bool,
    wifi_only: bool,
}

fn load_view(server_id: &str) -> Result<ViewLists, SessionError> {
    let db = database()?;
    let settings = db.settings()?;
    let invalid = setting_flag(&settings, &invalid_key(server_id));
    let wifi_only = setting_flag(&settings, "kavita.wifi_only")
        || settings.iter().all(|(key, _)| key != "kavita.wifi_only");
    let mut mirror = visible_kavita_mirror(db, server_id)?;
    let mut shelf = |name: &str| mirror.shelves.remove(name).unwrap_or_default();
    let (on_deck, added, updated) = (shelf("on-deck"), shelf("added"), shelf("updated"));
    Ok(ViewLists {
        series: mirror.series,
        libraries: mirror.libraries,
        home: mirror.home,
        on_deck,
        added,
        updated,
        wants: mirror.wants,
        bookmarks: mirror.bookmarks,
        downloads: kavita_downloads(db)?
            .into_iter()
            .filter(|row| row.server_id == server_id)
            .collect(),
        invalid_cert: invalid,
        wifi_only,
    })
}

const COVER_LIMIT: usize = 120;
const COVER_BATCH: usize = 8;

/// Series ids whose covers the current panel shows, in screen order.
fn cover_candidates(guard: &AppModel) -> Vec<i64> {
    let view = &guard.kavita;
    if view.panel == "home" {
        return view
            .on_deck
            .iter()
            .chain(&view.added)
            .chain(&view.updated)
            .copied()
            .collect();
    }
    shelf_page(guard, &view.panel)
        .map(|shelf| shelf.shown().to_vec())
        .unwrap_or_default()
}

/// Fetch the covers on screen, a batch at a time, until none is missing. One thread runs at a
/// time and re-reads what is on screen after every batch, so a panel change redirects it.
///
/// It yields to a sync, which needs the whole link for its 15 MiB: it does not start while one
/// runs and stops after the batch in flight when one begins. The sync's closing
/// `refresh_kavita` starts it again.
fn fetch_covers(mut model: ModelSignal) {
    let server_id = {
        let mut guard = model.write();
        if guard.kavita.covering || guard.kavita.busy || guard.kavita.server_id.is_empty() {
            return;
        }
        guard.kavita.covering = true;
        guard.kavita.server_id.clone()
    };
    std::thread::spawn(move || {
        let (base, key, allow) = match account(&server_id) {
            Ok(account) => account,
            Err(error) => {
                let mut guard = model.write();
                guard.kavita.covering = false;
                guard.status = error.to_string();
                return;
            }
        };
        loop {
            // The flag is cleared under the guard that found nothing left, so a panel change
            // either sees it still set (and this loop picks the new panel up) or starts again.
            let wanted = {
                let mut guard = model.write();
                let wanted = if guard.kavita.on
                    && !guard.kavita.busy
                    && guard.kavita.server_id == server_id
                {
                    covers_to_fetch(&cover_candidates(&guard), &guard.kavita.covers, COVER_LIMIT)
                } else {
                    Vec::new()
                };
                if wanted.is_empty() {
                    guard.kavita.covering = false;
                }
                wanted
            };
            if wanted.is_empty() {
                return;
            }
            let fetched = wanted
                .iter()
                .take(COVER_BATCH)
                .map(|id| {
                    // A failed fetch is stored empty so it is not asked for again this session.
                    let url = fetch_cover(&base, &key, *id, allow)
                        .map(|bytes| bytes_data_url(image_name(&bytes), &bytes))
                        .unwrap_or_default();
                    (*id, url)
                })
                .collect::<Vec<_>>();
            let mut guard = model.write();
            for (id, url) in fetched {
                guard.kavita.covers.insert(id, url);
            }
        }
    });
}

fn sync_accounts_now() -> Result<String, SessionError> {
    let db = database()?;
    let local = sync_now(db)?;
    let mut delivered = false;
    let mut push_failed = false;
    let mut fetch_error = None;
    let mut fetched = false;
    let mut saw_kavita = false;
    let mut gaps = Vec::new();
    for server in servers(db)? {
        if server.kind != "kavita" {
            continue;
        }
        saw_kavita = true;
        let store = SecretStore::open(&app_files_dir())?;
        let key = match read_key(&store, &server.id) {
            Ok(key) => key,
            Err(error) => {
                fetch_error = Some(connection_failure(&error));
                continue;
            }
        };
        let allow = invalid_for(db, &server.id)?;
        match push_pending(db, &server.id, &server.url, &key, allow) {
            Ok(true) => delivered = true,
            Ok(false) => {}
            Err(_) => push_failed = true,
        }
        match fetch_and_publish(db, &store, &server.id, &server.url, &key, allow) {
            Ok(found) => {
                fetched = true;
                gaps.extend(found.note());
            }
            Err(error) => fetch_error = Some(connection_failure(&error)),
        }
    }
    let failure = if fetched {
        None
    } else {
        fetch_error.as_deref()
    };
    let gap_note = (!gaps.is_empty()).then(|| gaps.join(" "));
    Ok(refresh_status(
        saw_kavita,
        delivered,
        failure,
        push_failed,
        gap_note.as_deref(),
        &local.detail,
    ))
}

fn account(server_id: &str) -> Result<(String, String, bool), SessionError> {
    let db = database()?;
    let server = servers(db)?
        .into_iter()
        .find(|server| server.id == server_id)
        .ok_or(SessionError::Server)?;
    let store = SecretStore::open(&app_files_dir())?;
    let key = read_key(&store, server_id)?;
    let allow = invalid_for(db, server_id)?;
    Ok((server.url, key, allow))
}

fn invalid_for(db: &folio_storage::Database, server_id: &str) -> Result<bool, SessionError> {
    Ok(setting_flag(&db.settings()?, &invalid_key(server_id)))
}

fn invalid_key(server_id: &str) -> String {
    let mut key = String::from("kavita.invalid_cert.");
    key.push_str(server_id);
    key
}

fn setting_flag(settings: &[(String, String)], key: &str) -> bool {
    settings
        .iter()
        .find(|(stored, _)| stored == key)
        .is_some_and(|(_, value)| value == "on")
}

fn store_pref(mut model: ModelSignal, key: &str, value: &str) {
    if let Ok(db) = database() {
        if let Err(error) = save_setting(db, key, value) {
            model.write().status = error.to_string();
        }
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

fn align_from(value: &str) -> TextAlign {
    match value {
        "center" => TextAlign::Center,
        "justify" => TextAlign::Justify,
        _ => TextAlign::Start,
    }
}

fn zoom_from(value: u8) -> SessionZoom {
    match value {
        2 => SessionZoom::Twice,
        4 => SessionZoom::Four,
        _ => SessionZoom::Fit,
    }
}

fn image_style(fit: &str, zoom: u8) -> String {
    let factor = zoom_factor(zoom_from(zoom));
    if factor == 1 {
        let css = image_fit_css(fit_from(fit));
        format!("pointer-events:none;{css}")
    } else {
        let width = u32::from(factor).saturating_mul(100);
        format!("width:{width}%;max-width:none;height:auto;object-fit:contain;pointer-events:none;")
    }
}

fn fit_from(value: &str) -> Fit {
    match value {
        "width" => Fit::Width,
        "height" => Fit::Height,
        "original" => Fit::Original,
        "smart" => Fit::Smart,
        _ => Fit::Screen,
    }
}

fn image_name(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "cover.png"
    } else if bytes.starts_with(b"RIFF") {
        "cover.webp"
    } else if bytes.starts_with(b"GIF8") {
        "cover.gif"
    } else {
        "cover.jpg"
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

fn link_unmetered() -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return true;
    };
    let mut wifi = false;
    let mut cell = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "lo" {
            continue;
        }
        let state = fs::read_to_string(entry.path().join("operstate")).unwrap_or_default();
        if !state.trim().eq_ignore_ascii_case("up") {
            continue;
        }
        if name.starts_with("wlan")
            || name.starts_with("wifi")
            || name.starts_with("eth")
            || name.starts_with("en")
        {
            wifi = true;
        }
        if name.starts_with("rmnet")
            || name.starts_with("ccmni")
            || name.starts_with("wwan")
            || name.starts_with("pdp")
        {
            cell = true;
        }
    }
    if wifi {
        return true;
    }
    !cell
}

fn chapter_kind(id: &str, chapters: &[(String, String, i64)]) -> Option<(String, i64)> {
    chapters
        .iter()
        .find(|(stored, _, _)| stored == id)
        .map(|(_, kind, pages)| (kind.clone(), *pages))
}

fn visible_kavita_chapters_for(
    db: &folio_storage::Database,
    server_id: &str,
) -> Result<Vec<(String, String, i64)>, SessionError> {
    let series = visible_kavita_series(db, server_id)?;
    let mut rows = Vec::new();
    for row in series {
        for chapter in visible_kavita_chapters(db, server_id, row.series_id)? {
            rows.push((
                format!("kavita-{server_id}-{}", chapter.chapter_id),
                row.format.clone(),
                chapter.pages,
            ));
        }
    }
    Ok(rows)
}

struct RemotePaint {
    title: String,
    body: String,
    html: bool,
    failed: bool,
    page: i64,
    page_count: i64,
    menu: bool,
    hide_progress: bool,
    paper: String,
    font_px: u32,
    align: String,
    indent: bool,
    follow_font: bool,
    fit: String,
    zoom: u8,
    mode: String,
}

const PAPERS: [(&str, &str); 4] = [
    ("light", "Light"),
    ("sepia", "Sepia"),
    ("dark", "Dark"),
    ("amoled", "Black"),
];

const ZONE: &str = "flex:1;min-width:48px;height:100%;margin:0;padding:0;border:0;outline:none;background:transparent;color:transparent;";
