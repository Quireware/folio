//! Reader shell. Compiled only for desktop and mobile targets.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use dioxus::prelude::*;
use folio_platform::SecretStore;
use folio_reader::{reading_action, ReadDirection, Side, TapZones, ZoneAction};

use crate::paginate::{boox_window, paginate};
use folio_storage::{CatalogItem, ReadingStats, StoredServer};

use crate::chrome::{theme_attr, CHROME_CSS};
use crate::kavita::{add_kavita, forget_server, update_kavita};
use crate::session::{
    add_server, app_files_dir, begin_session, browse_message, chapter_locator, cover_dir,
    edit_server, end_session, library_parent, load_snapshot, open_at, open_database, page_data_url,
    page_locator, place_index, record_page_count, save_bookmark, save_place, save_setting,
    scan_libraries, LibrarySnapshot, OpenedBook, SessionError,
};
use crate::shell::{self, Route};

mod icons;
mod kavita;
mod library;
mod places;
mod reading;

use library::library_screen;
use places::{history_screen, servers_screen, settings_screen, statistics_screen};
use reading::reading_screen;

/// The writer thread inside `Database` runs one job at a time, so callers share it without a
/// lock; anything that must not interleave with another caller is a single storage job.
static DATABASE: OnceLock<folio_storage::Database> = OnceLock::new();

type ModelSignal = Signal<AppModel, SyncStorage>;

struct AppModel {
    route: Route,
    status: String,
    scanning: bool,
    opening: bool,
    query: String,
    items: Vec<CatalogItem>,
    progress: Vec<crate::shell::ShelfProgress>,
    history: Vec<String>,
    servers: Vec<StoredServer>,
    stats: ReadingStats,
    appearance: String,
    paper: String,
    font_px: u32,
    menu: bool,
    server_name: String,
    server_url: String,
    server_kind: String,
    server_secret: String,
    /// Id of the saved server the Servers form is editing. Empty means the form adds one.
    server_editing: String,
    open: Option<OpenState>,
    /// Book id to a JPEG data URL. An empty string means the book has no cover.
    covers: BTreeMap<String, String>,
    covering: bool,
    kavita: kavita::KavitaView,
    remote: Option<kavita::RemoteRead>,
    remote_epoch: u64,
    align: String,
    indent: bool,
    hide_progress: bool,
    follow_font: bool,
    fit: String,
    reading_mode: String,
    zoom: u8,
}

struct OpenState {
    book_id: String,
    session_id: String,
    index: usize,
    /// Screen within the current text chapter. Pictures stay at 0.
    screen: usize,
    pages_turned: i64,
    book: OpenedBook,
    picture: Option<String>,
}

struct Place {
    locator: String,
    page: u32,
    chapter: String,
    complete: bool,
}

#[derive(Clone, Copy)]
enum Direction {
    Previous,
    Next,
}

impl AppModel {
    fn new() -> Self {
        Self {
            route: Route::Library,
            status: String::new(),
            scanning: false,
            opening: false,
            query: String::new(),
            items: Vec::new(),
            progress: Vec::new(),
            history: Vec::new(),
            servers: Vec::new(),
            stats: ReadingStats {
                streak_days: 0,
                pages_last_30_days: 0,
                formats: Vec::new(),
            },
            appearance: "system".to_owned(),
            paper: "light".to_owned(),
            font_px: 20,
            menu: false,
            server_name: String::new(),
            server_url: String::new(),
            server_kind: "opds".to_owned(),
            server_secret: String::new(),
            server_editing: String::new(),
            open: None,
            covers: BTreeMap::new(),
            covering: false,
            kavita: kavita::KavitaView::new(),
            remote: None,
            remote_epoch: 0,
            align: "start".to_owned(),
            indent: false,
            hide_progress: false,
            follow_font: false,
            fit: "screen".to_owned(),
            reading_mode: "paginated".to_owned(),
            zoom: 1,
        }
    }
}

/// Open the reader.
pub fn launch() {
    dioxus::launch(app);
}

fn app() -> Element {
    let model = use_signal_sync(AppModel::new);
    use_hook(|| start_scan(model));
    let remote = model.read().remote.is_some();
    let open = remote || model.read().open.is_some();
    let route = model.read().route;
    let theme = theme_attr(&model.read().appearance);
    let body = if remote {
        kavita::remote_reading(model)
    } else if open {
        reading_screen(model)
    } else {
        match route {
            Route::Library => library_screen(model),
            Route::Statistics => statistics_screen(model),
            Route::Settings => settings_screen(model),
            Route::History => history_screen(model),
            Route::Servers => servers_screen(model),
        }
    };
    rsx! {
        style { "{CHROME_CSS}" }
        div {
            class: "f-app",
            "data-theme": "{theme}",
            {body}
            if !open {
                {tab_bar(model)}
            }
        }
    }
}

fn tab_bar(model: ModelSignal) -> Element {
    let route = model.read().route;
    rsx! {
        nav {
            class: "f-nav",
            for (name, label) in shell::chrome_labels().iter().copied() {
                button {
                    key: "{name}",
                    class: "f-tab",
                    aria_current: current(route_name_is(route, name)),
                    onclick: move |_| set_route(model, route_from_name(name)),
                    span { class: "f-tab__icon", {icons::nav_icon(name)} }
                    span { class: "f-tab__label", "{label}" }
                }
            }
        }
    }
}

/// `aria-pressed` value for a segmented or chip control.
fn pressed(active: bool) -> &'static str {
    if active {
        "true"
    } else {
        "false"
    }
}

fn current(active: bool) -> &'static str {
    if active {
        "page"
    } else {
        "false"
    }
}

fn start_scan(mut model: ModelSignal) {
    {
        let mut guard = model.write();
        if guard.scanning {
            return;
        }
        guard.scanning = true;
        guard.status = "Scanning".to_owned();
    }
    std::thread::spawn(move || {
        if let Err(error) = scan_worker(model) {
            let mut guard = model.write();
            guard.scanning = false;
            guard.status = error.to_string();
        }
        fill_covers(model);
    });
}

fn scan_worker(model: ModelSignal) -> Result<(), SessionError> {
    ensure_database()?;
    let recover_note = match kavita::recover_kavita_downloads() {
        Ok(()) => None,
        Err(error) => Some(error.to_string()),
    };
    let (moved, rehome_note) = rehome_saved_server();
    let parent = library_parent();
    if !parent.is_dir() {
        reload_quiet(model);
        finish_scan(
            model,
            moved,
            format!("Library folder is not available: {}.", parent.display()),
            recover_note,
            rehome_note,
        );
        return Ok(());
    }
    let files = app_files_dir();
    let scan = {
        let db = database()?;
        scan_libraries(&parent, db, &cover_dir(&files))?
    };
    let count = scan.outcome.rows.len();
    let unreadable = scan.unreadable;
    let snapshot = {
        let db = database()?;
        load_snapshot(db)?
    };
    apply_snapshot(model, snapshot);
    finish_scan(
        model,
        moved,
        ready_status(count, &unreadable),
        recover_note,
        rehome_note,
    );
    Ok(())
}

fn rehome_saved_server() -> (Option<String>, Option<String>) {
    let result = (|| {
        let db = database()?;
        let store = SecretStore::open(&app_files_dir())?;
        crate::kavita::rehome_raw_opds(db, &store)
    })();
    match result {
        Ok(id) => {
            let note = id
                .as_ref()
                .map(|_| "The saved server is Kavita.".to_owned());
            (id, note)
        }
        Err(error) => (None, Some(crate::kavita::connection_failure(&error))),
    }
}

fn finish_scan(
    mut model: ModelSignal,
    moved: Option<String>,
    status: String,
    recover_note: Option<String>,
    rehome_note: Option<String>,
) {
    let status = with_note(status, recover_note.as_deref());
    let status = with_note(status, rehome_note.as_deref());
    let start_sync = moved.is_some();
    {
        let mut guard = model.write();
        if let Some(id) = moved {
            guard.kavita.on = true;
            guard.kavita.server_id = id;
        }
        guard.scanning = false;
        guard.status = status;
    }
    if start_sync {
        kavita::sync_accounts(model);
    }
}

fn reload_quiet(model: ModelSignal) {
    let Ok(db) = database() else {
        return;
    };
    let Ok(snapshot) = load_snapshot(db) else {
        return;
    };
    apply_snapshot(model, snapshot);
}

fn with_note(status: String, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{status} {note}"),
        None => status,
    }
}

fn cover_queue(model: &ModelSignal) -> Vec<(String, String)> {
    let guard = model.read();
    let ids = guard
        .items
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let mut seen = std::collections::BTreeSet::new();
    let mut queue = Vec::new();
    for shelf in shell::home_shelves(&guard.progress, &ids) {
        if shelf.name == "recently-read" {
            continue;
        }
        let mut book_ids = shelf.book_ids;
        if shelf.name == "catalog" {
            book_ids.sort_by(|left, right| {
                title_of(&guard.items, left)
                    .to_ascii_lowercase()
                    .cmp(&title_of(&guard.items, right).to_ascii_lowercase())
            });
        }
        for id in book_ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            let kind = guard
                .items
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.file_type.clone())
                .unwrap_or_default();
            queue.push((id, kind));
        }
    }
    queue
}

fn fill_covers(mut model: ModelSignal) {
    {
        let mut guard = model.write();
        if guard.covering {
            return;
        }
        guard.covering = true;
    }
    let dir = cover_dir(&app_files_dir());
    let library = library_parent();
    loop {
        let items = cover_queue(&model);
        let mut made_progress = false;
        for (id, kind) in items {
            if model.read().covers.contains_key(&id) {
                continue;
            }
            while {
                let guard = model.read();
                guard.opening || guard.open.is_some()
            } {
                std::thread::sleep(Duration::from_millis(200));
            }
            if model.read().covers.contains_key(&id) {
                continue;
            }
            let url = match crate::covers::materialise(&dir, &id, &library, &kind) {
                Ok(url) => url.unwrap_or_default(),
                Err(error) => {
                    tracing::debug!(%error, %id, "cover was not stored");
                    String::new()
                }
            };
            model.write().covers.insert(id, url);
            made_progress = true;
        }
        if !made_progress {
            break;
        }
    }
    model.write().covering = false;
}

fn open_book(mut model: ModelSignal, book_id: String) {
    let file_type = {
        let guard = model.read();
        guard
            .items
            .iter()
            .find(|item| item.id == book_id)
            .map(|item| item.file_type.clone())
    };
    let Some(file_type) = file_type else {
        model.write().status = "That book is not in the library.".to_owned();
        return;
    };
    {
        let mut guard = model.write();
        if guard.opening {
            return;
        }
        guard.opening = true;
        guard.status = "Opening".to_owned();
    }
    std::thread::spawn(move || {
        if let Err(error) = ensure_database() {
            let mut guard = model.write();
            guard.opening = false;
            guard.status = error.to_string();
            return;
        }
        let parent = library_parent();
        let opened = open_at(&parent, &book_id, &file_type);
        apply_open(model, book_id, opened);
    });
}

fn apply_open(mut model: ModelSignal, book_id: String, opened: Result<OpenedBook, SessionError>) {
    let book = match opened {
        Ok(book) => book,
        Err(error) => {
            let mut guard = model.write();
            guard.opening = false;
            guard.status = error.to_string();
            return;
        }
    };
    let prepared = match database() {
        Ok(db) => prepare_open(db, &book_id, &book),
        Err(error) => Err(error),
    };
    if prepared.is_ok() {
        store_page_count(model, &book_id, book.len());
    }
    let mut guard = model.write();
    guard.opening = false;
    match prepared {
        Ok((session_id, index)) => {
            let (picture, draw_error) = drawn_picture(&book, index);
            match draw_error {
                Some(message) => guard.status = message,
                None => guard.status.clear(),
            }
            guard.menu = false;
            guard.open = Some(OpenState {
                book_id,
                session_id,
                index,
                screen: 0,
                pages_turned: 0,
                book,
                picture,
            });
        }
        Err(error) => {
            let (picture, draw_error) = drawn_picture(&book, 0);
            guard.status = draw_error.unwrap_or_else(|| error.to_string());
            guard.menu = false;
            guard.open = Some(OpenState {
                book_id,
                session_id: String::new(),
                index: 0,
                screen: 0,
                pages_turned: 0,
                book,
                picture,
            });
        }
    }
}

/// The scan never opens books, so the count is learned here; `close_book` reloads it into the shelf.
fn store_page_count(model: ModelSignal, book_id: &str, count: usize) {
    let Some(item) = model
        .read()
        .items
        .iter()
        .find(|item| item.id == book_id)
        .cloned()
    else {
        return;
    };
    if let Err(error) = database().and_then(|db| record_page_count(db, &item, count)) {
        // A missing count only hides the Continue bar; it must not stop the book opening.
        tracing::warn!(%error, "page count was not stored");
    }
}

fn prepare_open(
    db: &folio_storage::Database,
    book_id: &str,
    book: &OpenedBook,
) -> Result<(String, usize), SessionError> {
    let saved = db.reading(book_id)?;
    let mut index = saved
        .as_ref()
        .and_then(|row| place_index(&row.locator))
        .unwrap_or(0);
    let len = book.len();
    if len == 0 {
        index = 0;
    } else if index >= len {
        index = len.saturating_sub(1);
    }
    let session_id = begin_session(db, book_id)?;
    let place = place_at(book, index)?;
    save_place(
        db,
        book_id,
        &place.locator,
        place.page,
        &place.chapter,
        place.complete,
    )?;
    Ok((session_id, index))
}

fn close_book(mut model: ModelSignal) {
    if model.read().remote.is_some() {
        kavita::close_remote(model);
        return;
    }
    let (session_id, pages) = {
        let guard = model.read();
        match &guard.open {
            Some(open) => (open.session_id.clone(), open.pages_turned),
            None => return,
        }
    };
    {
        let mut guard = model.write();
        guard.open = None;
        guard.menu = false;
    }
    if session_id.is_empty() {
        return;
    }
    if let Ok(db) = database() {
        if let Err(error) = end_session(db, &session_id, pages) {
            model.write().status = error.to_string();
            return;
        }
    }
    reload(model);
}

fn zone_tapped(model: ModelSignal, side: Side) {
    if model.read().remote.is_some() {
        match reading_action(side, ReadDirection::Ltr, TapZones::EDGES) {
            ZoneAction::Previous => kavita::turn_remote(model, -1),
            ZoneAction::Next => kavita::turn_remote(model, 1),
            ZoneAction::Menu => toggle_menu(model),
            ZoneAction::Nothing => {}
        }
        return;
    }
    match reading_action(side, ReadDirection::Ltr, TapZones::EDGES) {
        ZoneAction::Previous => turn(model, Direction::Previous),
        ZoneAction::Next => turn(model, Direction::Next),
        ZoneAction::Menu => toggle_menu(model),
        ZoneAction::Nothing => {}
    }
}

fn toggle_menu(mut model: ModelSignal) {
    let menu = model.read().menu;
    model.write().menu = !menu;
}

fn turn(model: ModelSignal, direction: Direction) {
    let decision = {
        let guard = model.read();
        let Some(open) = guard.open.as_ref() else {
            return;
        };
        match &open.book {
            OpenedBook::Text { .. } if guard.reading_mode == "scroll" => {
                chapter_decision(open, direction)
            }
            OpenedBook::Text { .. } => text_decision(open, direction, guard.font_px),
            OpenedBook::Pages { .. } | OpenedBook::Pdf { .. } => index_decision(open, direction),
        }
    };
    apply_decision(model, direction, decision);
}

enum Decision {
    Stay,
    Failed(String),
    Screen(usize),
    Jump {
        book_id: String,
        next: usize,
        screen: usize,
        place: Place,
    },
}

fn chapter_decision(open: &OpenState, direction: Direction) -> Decision {
    match direction {
        Direction::Next => jump_chapter(open, open.index.saturating_add(1), 0),
        Direction::Previous => {
            if open.index == 0 {
                Decision::Stay
            } else {
                jump_chapter(open, open.index.saturating_sub(1), 0)
            }
        }
    }
}

fn text_decision(open: &OpenState, direction: Direction, font_px: u32) -> Decision {
    let window = boox_window(font_px);
    let pages = paginate(open.book.blocks(open.index), window);
    let last = pages.len().saturating_sub(1);
    let screen = open.screen.min(last);
    match direction {
        Direction::Next => {
            if !pages.is_empty() && screen < last {
                return Decision::Screen(screen.saturating_add(1));
            }
            jump_chapter(open, open.index.saturating_add(1), 0)
        }
        Direction::Previous => {
            if screen > 0 {
                return Decision::Screen(screen.saturating_sub(1));
            }
            if open.index == 0 {
                return Decision::Stay;
            }
            let next = open.index.saturating_sub(1);
            let screen = paginate(open.book.blocks(next), window)
                .len()
                .saturating_sub(1);
            jump_chapter(open, next, screen)
        }
    }
}

fn jump_chapter(open: &OpenState, next: usize, screen: usize) -> Decision {
    if next >= open.book.len() || next == open.index {
        return Decision::Stay;
    }
    match place_at(&open.book, next) {
        Ok(place) => Decision::Jump {
            // clone: the chapter change is applied after this borrow ends
            book_id: open.book_id.clone(),
            next,
            screen,
            place,
        },
        Err(error) => Decision::Failed(error.to_string()),
    }
}

fn index_decision(open: &OpenState, direction: Direction) -> Decision {
    let len = open.book.len();
    if len == 0 {
        return Decision::Stay;
    }
    let next = match direction {
        Direction::Previous => open.index.saturating_sub(1),
        Direction::Next => open.index.saturating_add(1).min(len.saturating_sub(1)),
    };
    if next == open.index {
        return Decision::Stay;
    }
    match place_at(&open.book, next) {
        Ok(place) => Decision::Jump {
            // clone: the page change is applied after this borrow ends
            book_id: open.book_id.clone(),
            next,
            screen: 0,
            place,
        },
        Err(error) => Decision::Failed(error.to_string()),
    }
}

fn apply_decision(mut model: ModelSignal, direction: Direction, decision: Decision) {
    match decision {
        Decision::Stay => {}
        Decision::Failed(message) => model.write().status = message,
        Decision::Screen(screen) => {
            let mut guard = model.write();
            if let Some(open) = guard.open.as_mut() {
                open.screen = screen;
                if matches!(direction, Direction::Next) {
                    open.pages_turned = open.pages_turned.saturating_add(1);
                }
            }
        }
        Decision::Jump {
            book_id,
            next,
            screen,
            place,
        } => {
            if let Ok(db) = database() {
                if let Err(error) = save_place(
                    db,
                    &book_id,
                    &place.locator,
                    place.page,
                    &place.chapter,
                    place.complete,
                ) {
                    model.write().status = error.to_string();
                    return;
                }
            }
            let drawn = {
                let guard = model.read();
                guard
                    .open
                    .as_ref()
                    .filter(|open| open.book_id == book_id)
                    .map(|open| drawn_picture(&open.book, next))
            };
            let Some((picture, draw_error)) = drawn else {
                return;
            };
            let drew_pdf = picture.is_some();
            let mut guard = model.write();
            if let Some(open) = guard.open.as_mut() {
                if open.book_id == book_id {
                    open.index = next;
                    open.screen = screen;
                    if matches!(open.book, OpenedBook::Pdf { .. }) {
                        open.picture = picture;
                    }
                    if matches!(direction, Direction::Next) {
                        open.pages_turned = open.pages_turned.saturating_add(1);
                    }
                }
            }
            if let Some(message) = draw_error {
                guard.status = message;
            } else if drew_pdf {
                guard.status.clear();
            }
        }
    }
}

fn drawn_picture(book: &OpenedBook, index: usize) -> (Option<String>, Option<String>) {
    match pdf_picture(book, index) {
        Ok(picture) => (picture, None),
        Err(error) => (None, Some(error.to_string())),
    }
}

fn pdf_picture(book: &OpenedBook, index: usize) -> Result<Option<String>, SessionError> {
    if !matches!(book, OpenedBook::Pdf { .. }) {
        return Ok(None);
    }
    let bytes = book.pdf_png(index)?;
    Ok(Some(page_data_url("page.png", &bytes)))
}

fn change_font(model: ModelSignal, larger: bool) {
    let current = model.read().font_px;
    let next = if larger {
        current.saturating_add(2)
    } else {
        current.saturating_sub(2)
    };
    set_font(model, next);
}

fn set_font(mut model: ModelSignal, font_px: u32) {
    let font_px = font_px.clamp(1, 96);
    if let Ok(db) = database() {
        if let Err(error) = save_setting(db, "reader.font_px", &font_px.to_string()) {
            model.write().status = error.to_string();
            return;
        }
    }
    model.write().font_px = font_px;
}

fn set_paper(mut model: ModelSignal, value: String) {
    if !known_paper(&value) {
        return;
    }
    if let Ok(db) = database() {
        if let Err(error) = save_setting(db, "reader.paper", &value) {
            model.write().status = error.to_string();
            return;
        }
    }
    model.write().paper = value;
}

fn set_appearance(mut model: ModelSignal, value: String) {
    if !known_appearance(&value) {
        return;
    }
    if let Ok(db) = database() {
        if let Err(error) = save_setting(db, "appearance", &value) {
            model.write().status = error.to_string();
            return;
        }
    }
    model.write().appearance = value;
}

fn bookmark(mut model: ModelSignal) {
    let (book_id, place) = {
        let guard = model.read();
        let Some(open) = guard.open.as_ref() else {
            return;
        };
        match place_at(&open.book, open.index) {
            Ok(place) => (open.book_id.clone(), place),
            Err(error) => {
                drop(guard);
                model.write().status = error.to_string();
                return;
            }
        }
    };
    if let Ok(db) = database() {
        match save_bookmark(db, &book_id, &place.locator, "Bookmark") {
            Ok(()) => model.write().status = "Bookmark saved.".to_owned(),
            Err(error) => model.write().status = error.to_string(),
        }
    }
}

fn sync_clicked(model: ModelSignal) {
    kavita::sync_accounts(model);
}

fn add_server_clicked(mut model: ModelSignal) {
    let (kind, url, name, secret) = {
        let guard = model.read();
        (
            guard.server_kind.clone(),
            guard.server_url.clone(),
            guard.server_name.clone(),
            guard.server_secret.clone(),
        )
    };
    if kind == "kavita" {
        if let Some(status) = kavita_form_problem(&url, &secret) {
            model.write().status = status;
            return;
        }
    }
    let saved = (|| -> Result<Option<String>, SessionError> {
        let db = database()?;
        if kind == "kavita" {
            let store = SecretStore::open(&app_files_dir())?;
            Ok(Some(add_kavita(db, &store, &name, &url, &secret)?))
        } else {
            add_server(db, &kind, &url, &name, &secret)?;
            Ok(None)
        }
    })();
    match saved {
        Ok(new_id) => {
            let open_kavita = new_id.is_some();
            {
                let mut guard = model.write();
                guard.server_name.clear();
                guard.server_url.clear();
                guard.server_secret.clear();
                if let Some(id) = new_id {
                    guard.kavita.on = true;
                    guard.kavita.server_id = id;
                    guard.route = Route::Library;
                }
                guard.status = "Server saved.".to_owned();
            }
            reload(model);
            if open_kavita {
                kavita::sync_accounts(model);
            }
        }
        Err(error) => {
            model.write().status = if kind == "kavita" {
                crate::kavita::connection_failure(&error)
            } else {
                error.to_string()
            };
        }
    }
}

fn edit_server_clicked(mut model: ModelSignal, id: String) {
    let row = model
        .read()
        .servers
        .iter()
        .find(|server| server.id == id)
        .map(|server| {
            (
                server.kind.clone(),
                server.display_name.clone(),
                server.url.clone(),
            )
        });
    let Some((kind, name, url)) = row else {
        return;
    };
    {
        let mut guard = model.write();
        guard.server_editing = id;
        guard.server_kind = kind;
        guard.server_name = name;
        guard.server_url = url;
        guard.server_secret.clear();
        guard.status.clear();
    }
    // The form is at the top of the screen and the Edit buttons are below it.
    dioxus::document::eval("window.scrollTo(0, 0);");
}

fn cancel_edit_clicked(mut model: ModelSignal) {
    let mut guard = model.write();
    guard.server_editing.clear();
    guard.server_name.clear();
    guard.server_url.clear();
    guard.server_secret.clear();
}

fn save_edit_clicked(mut model: ModelSignal) {
    let (id, kind, name, url, secret) = {
        let guard = model.read();
        (
            guard.server_editing.clone(),
            guard.server_kind.clone(),
            guard.server_name.clone(),
            guard.server_url.clone(),
            guard.server_secret.clone(),
        )
    };
    let kavita = kind == "kavita";
    if kavita && !(url.starts_with("http://") || url.starts_with("https://")) {
        model.write().status = "Type a URL that starts with http:// or https://.".to_owned();
        return;
    }
    let saved = (|| {
        let db = database()?;
        if kavita {
            let store = SecretStore::open(&app_files_dir())?;
            update_kavita(db, &store, &id, &name, &url, &secret)
        } else {
            edit_server(db, &id, &name, &url, &secret)
        }
    })();
    match saved {
        Ok(()) => {
            cancel_edit_clicked(model);
            model.write().status = "Server saved.".to_owned();
            reload(model);
            if model.read().kavita.server_id == id {
                kavita::refresh_kavita(model);
            }
        }
        Err(SessionError::NameTaken) => {
            model.write().status = "A server with that name is already saved.".to_owned();
        }
        Err(error) => {
            model.write().status = if kavita {
                crate::kavita::connection_failure(&error)
            } else {
                error.to_string()
            };
        }
    }
}

fn test_edit_clicked(model: ModelSignal) {
    let (id, url, secret) = {
        let guard = model.read();
        (
            guard.server_editing.clone(),
            guard.server_url.clone(),
            guard.server_secret.clone(),
        )
    };
    let key = if secret.is_empty() {
        None
    } else {
        Some(secret)
    };
    test_address(model, url, key, id);
}

fn test_connection_clicked(mut model: ModelSignal) {
    let (kind, url, secret, saved) = {
        let guard = model.read();
        let saved = guard
            .servers
            .iter()
            .find(|server| server.kind == "kavita")
            .map(|server| (server.id.clone(), server.url.clone()));
        (
            guard.server_kind.clone(),
            guard.server_url.clone(),
            guard.server_secret.clone(),
            saved,
        )
    };
    if !(url.is_empty() || url.starts_with("http://") || url.starts_with("https://")) {
        model.write().status = "Type a URL that starts with http:// or https://.".to_owned();
        return;
    }
    if !secret.is_empty() && kind != "kavita" {
        model.write().status = "Tap Kavita first.".to_owned();
        return;
    }
    let stored = saved.filter(|_| secret.is_empty());
    if secret.is_empty() && stored.is_none() && url.is_empty() {
        model.write().status = "Type a URL that starts with http:// or https://.".to_owned();
        return;
    }
    if secret.is_empty() && stored.is_none() {
        model.write().status = "The Auth Key is missing.".to_owned();
        return;
    }
    let target = if url.is_empty() {
        stored
            .as_ref()
            .map(|(_, saved_url)| saved_url.clone())
            .unwrap_or_default()
    } else {
        url
    };
    model.write().status = "Contacting Kavita.".to_owned();
    std::thread::spawn(move || {
        let key = if secret.is_empty() {
            stored
                .as_ref()
                .map(|(id, _)| saved_key(id))
                .unwrap_or(Err(SessionError::Server))
        } else {
            Ok(secret)
        };
        let message = match key {
            Ok(key) => probe_message(&target, &key),
            Err(error) => crate::kavita::connection_failure(&error),
        };
        model.write().status = message;
    });
}

fn test_address(mut model: ModelSignal, url: String, key: Option<String>, server_id: String) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        model.write().status = "Type a URL that starts with http:// or https://.".to_owned();
        return;
    }
    model.write().status = "Contacting Kavita.".to_owned();
    std::thread::spawn(move || {
        let key = match key {
            Some(key) => Ok(key),
            None => saved_key(&server_id),
        };
        let message = match key {
            Ok(key) => probe_message(&url, &key),
            Err(error) => crate::kavita::connection_failure(&error),
        };
        model.write().status = message;
    });
}

fn test_saved_clicked(mut model: ModelSignal, id: String) {
    let url = {
        let guard = model.read();
        guard
            .servers
            .iter()
            .find(|server| server.id == id)
            .map(|server| server.url.clone())
    };
    let Some(url) = url else {
        model.write().status = "The Auth Key is missing.".to_owned();
        return;
    };
    model.write().status = "Contacting Kavita.".to_owned();
    std::thread::spawn(move || {
        let message = match saved_key(&id) {
            Ok(key) => probe_message(&url, &key),
            Err(error) => crate::kavita::connection_failure(&error),
        };
        model.write().status = message;
    });
}

fn saved_key(server_id: &str) -> Result<String, SessionError> {
    let store = SecretStore::open(&app_files_dir())?;
    crate::kavita::read_key(&store, server_id)
}

fn probe_message(url: &str, key: &str) -> String {
    match crate::kavita::probe_kavita(url, key) {
        Ok(count) => format!("Connected. {count} libraries."),
        Err(error) => crate::kavita::connection_failure(&error),
    }
}

fn delete_server_clicked(mut model: ModelSignal, id: String) {
    let removed = (|| {
        let db = database()?;
        let files = app_files_dir();
        let store = SecretStore::open(&files)?;
        forget_server(db, &store, &files, &id)
    })();
    match removed {
        Ok(()) => {
            let active = model.read().kavita.server_id == id;
            if model
                .read()
                .remote
                .as_ref()
                .is_some_and(|remote| remote.server_id == id)
            {
                let mut guard = model.write();
                guard.remote = None;
                guard.menu = false;
            }
            model.write().status = "Server removed.".to_owned();
            reload(model);
            if active {
                let wifi_only = model.read().kavita.wifi_only;
                let mut cleared = kavita::KavitaView::new();
                cleared.wifi_only = wifi_only;
                model.write().kavita = cleared;
            }
        }
        Err(error) => model.write().status = error.to_string(),
    }
}

fn use_as_kavita_clicked(mut model: ModelSignal, id: String) {
    let saved = (|| {
        let db = database()?;
        let store = SecretStore::open(&app_files_dir())?;
        crate::kavita::rehome_server(db, &store, &id)
    })();
    match saved {
        Ok(new_id) => {
            {
                let mut guard = model.write();
                guard.kavita.on = true;
                guard.kavita.server_id = new_id;
                guard.route = Route::Library;
                guard.status = "The saved server is Kavita.".to_owned();
            }
            reload(model);
            kavita::sync_accounts(model);
        }
        Err(error) => model.write().status = crate::kavita::connection_failure(&error),
    }
}

fn kavita_form_problem(url: &str, secret: &str) -> Option<String> {
    if secret.is_empty() {
        return Some("The Auth Key is missing.".to_owned());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Some("Type a URL that starts with http:// or https://.".to_owned());
    }
    None
}

fn browse_clicked(mut model: ModelSignal, id: String) {
    let row = model
        .read()
        .servers
        .iter()
        .find(|server| server.id == id)
        .map(|server| {
            (
                server.kind.clone(),
                server.url.clone(),
                server.display_name.clone(),
                server.secret_ref.clone(),
            )
        });
    let Some((kind, url, name, secret)) = row else {
        return;
    };
    if kind == "kavita" {
        {
            let mut guard = model.write();
            guard.kavita.server_id = id;
            guard.route = Route::Library;
        }
        kavita::activate_kavita(model);
        return;
    }
    model.write().status = browse_message(&kind, &url, &name, secret.as_deref());
}

fn set_route(mut model: ModelSignal, route: Route) {
    close_book(model);
    // The Kavita Server panel fills the same form fields, so an edit does not outlive the tab.
    if !matches!(route, Route::Servers) && !model.read().server_editing.is_empty() {
        cancel_edit_clicked(model);
    }
    model.write().route = route;
}

fn reload(mut model: ModelSignal) {
    let snapshot = match database() {
        Ok(db) => match load_snapshot(db) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                model.write().status = error.to_string();
                return;
            }
        },
        Err(error) => {
            model.write().status = error.to_string();
            return;
        }
    };
    apply_snapshot(model, snapshot);
}

fn apply_snapshot(mut model: ModelSignal, snapshot: LibrarySnapshot) {
    let appearance = setting_value(&snapshot, "appearance").unwrap_or("system");
    let paper = setting_value(&snapshot, "reader.paper").unwrap_or("light");
    let font_px = setting_value(&snapshot, "reader.font_px")
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|size| (1..=96).contains(size))
        .unwrap_or(20);
    let appearance = if known_appearance(appearance) {
        appearance.to_owned()
    } else {
        "system".to_owned()
    };
    let paper = if known_paper(paper) {
        paper.to_owned()
    } else {
        "light".to_owned()
    };
    let align = match setting_value(&snapshot, "reader.align") {
        Some("center") => "center",
        Some("justify") => "justify",
        _ => "start",
    };
    let fit = match setting_value(&snapshot, "reader.fit") {
        Some("width") => "width",
        Some("height") => "height",
        Some("original") => "original",
        Some("smart") => "smart",
        _ => "screen",
    };
    let reading_mode = if setting_value(&snapshot, "reader.mode") == Some("scroll") {
        "scroll"
    } else {
        "paginated"
    };
    let indent = setting_value(&snapshot, "reader.indent") == Some("on");
    let hide_progress = setting_value(&snapshot, "reader.hide_progress") == Some("on");
    let follow_font = setting_value(&snapshot, "reader.follow_font") == Some("on");
    let wifi_only = setting_value(&snapshot, "kavita.wifi_only") != Some("off");
    let mut guard = model.write();
    guard.items = snapshot.items;
    guard.progress = snapshot.progress;
    guard.history = snapshot.history;
    guard.servers = snapshot.servers;
    guard.stats = snapshot.stats;
    guard.appearance = appearance;
    guard.paper = paper;
    guard.font_px = font_px;
    guard.align = align.to_owned();
    guard.indent = indent;
    guard.hide_progress = hide_progress;
    guard.follow_font = follow_font;
    guard.fit = fit.to_owned();
    guard.reading_mode = reading_mode.to_owned();
    guard.kavita.wifi_only = wifi_only;
}

fn setting_value<'a>(snapshot: &'a LibrarySnapshot, key: &str) -> Option<&'a str> {
    snapshot
        .settings
        .iter()
        .find(|(stored, _)| stored == key)
        .map(|(_, value)| value.as_str())
}

fn ensure_database() -> Result<(), SessionError> {
    if DATABASE.get().is_some() {
        return Ok(());
    }
    let db = open_database(&app_files_dir())?;
    if DATABASE.set(db).is_err() {
        return Ok(());
    }
    Ok(())
}

fn database() -> Result<&'static folio_storage::Database, SessionError> {
    DATABASE.get().ok_or(SessionError::Closed)
}

fn place_at(book: &OpenedBook, index: usize) -> Result<Place, SessionError> {
    let page = u32::try_from(index).map_err(|_| SessionError::Number)?;
    let complete = !book.is_empty() && index.saturating_add(1) == book.len();
    let locator = match book {
        OpenedBook::Pages { .. } | OpenedBook::Pdf { .. } => page_locator(index)?,
        OpenedBook::Text { .. } => chapter_locator(index)?,
    };
    Ok(Place {
        // clone: the chapter column is stored separately from the locator
        chapter: locator.clone(),
        locator,
        page,
        complete,
    })
}

fn ready_status(count: usize, unreadable: &[String]) -> String {
    let books = if count == 1 {
        "1 book".to_owned()
    } else {
        format!("{count} books")
    };
    if unreadable.is_empty() {
        books
    } else {
        format!("{books}. Could not read {}.", unreadable.join(", "))
    }
}

fn known_appearance(value: &str) -> bool {
    matches!(value, "system" | "light" | "sepia" | "dark" | "amoled")
}

fn known_paper(value: &str) -> bool {
    matches!(value, "light" | "sepia" | "dark" | "amoled")
}

fn route_from_name(name: &str) -> Route {
    match name {
        "statistics" => Route::Statistics,
        "settings" => Route::Settings,
        "history" => Route::History,
        "servers" => Route::Servers,
        _ => Route::Library,
    }
}

fn route_name_is(route: Route, name: &str) -> bool {
    shell::route_name(route) == name
}

fn title_of(items: &[CatalogItem], id: &str) -> String {
    items
        .iter()
        .find(|item| item.id == id)
        .map(|item| item.title.clone())
        .unwrap_or_else(|| id.to_owned())
}
