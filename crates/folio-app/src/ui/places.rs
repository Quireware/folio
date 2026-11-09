//! Statistics, settings, history, and servers. Settings has no test button.

use dioxus::prelude::*;

use crate::kavita::adoptable_opds;
use crate::settings::settings_actions;

use super::{
    add_server_clicked, browse_clicked, cancel_edit_clicked, change_font, delete_server_clicked,
    edit_server_clicked, pressed, save_edit_clicked, set_appearance, start_scan, sync_clicked,
    test_connection_clicked, test_edit_clicked, test_saved_clicked, title_of,
    use_as_kavita_clicked, ModelSignal,
};

const APPEARANCES: [(&str, &str); 5] = [
    ("system", "System"),
    ("light", "Light"),
    ("sepia", "Sepia"),
    ("dark", "Dark"),
    ("amoled", "Black"),
];

pub(super) fn statistics_screen(model: ModelSignal) -> Element {
    let status = model.read().status.clone();
    let stats = model.read().stats.clone();
    let formats = stats.formats.clone();
    rsx! {
        main {
            header {
                class: "f-page-head",
                h1 { class: "f-h1", "Statistics" }
                p { class: "f-status", "{status}" }
            }
            div {
                class: "f-tiles",
                div {
                    class: "f-tile",
                    p { class: "f-tile__value", "{stats.streak_days}" }
                    p { class: "f-tile__label", "Day streak" }
                }
                div {
                    class: "f-tile",
                    p { class: "f-tile__value", "{stats.pages_last_30_days}" }
                    p { class: "f-tile__label", "Pages in 30 days" }
                }
            }
            if !formats.is_empty() {
                h2 { class: "f-h2", "Formats" }
                ul {
                    class: "f-list",
                    for (kind, count) in formats {
                        li {
                            key: "{kind}",
                            class: "f-list__item f-list__item--split",
                            span { "{kind}" }
                            span { class: "f-meta-strong", "{count}" }
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn settings_screen(model: ModelSignal) -> Element {
    let status = model.read().status.clone();
    rsx! {
        main {
            header {
                class: "f-page-head",
                h1 { class: "f-h1", "Settings" }
                p { class: "f-status", "{status}" }
            }
            for action in settings_actions().iter().copied() {
                section {
                    key: "{action}",
                    class: "f-card",
                    h2 { class: "f-card__title", "{section_title(action)}" }
                    {section_body(model, action)}
                }
            }
        }
    }
}

pub(super) fn history_screen(model: ModelSignal) -> Element {
    let status = model.read().status.clone();
    let history = model.read().history.clone();
    let items = model.read().items.clone();
    rsx! {
        main {
            header {
                class: "f-page-head",
                h1 { class: "f-h1", "History" }
                p { class: "f-status", "{status}" }
            }
            if history.is_empty() {
                p { class: "f-empty", "No books opened yet." }
            } else {
                ul {
                    class: "f-list",
                    for id in history {
                        li { key: "{id}", class: "f-list__item", "{title_of(&items, &id)}" }
                    }
                }
            }
        }
    }
}

pub(super) fn servers_screen(mut model: ModelSignal) -> Element {
    let status = model.read().status.clone();
    let name = model.read().server_name.clone();
    let url = model.read().server_url.clone();
    let kind = model.read().server_kind.clone();
    let secret = model.read().server_secret.clone();
    let editing = !model.read().server_editing.is_empty();
    let secret_heading = if kind == "kavita" {
        "Auth Key"
    } else {
        "Secret key"
    };
    let secret_placeholder = match (editing, kind == "kavita") {
        (true, true) => "Leave blank to keep the saved key",
        (true, false) => "Leave blank to keep the saved secret key",
        (false, true) => "Leave blank only when editing a saved server",
        (false, false) => "keyring:komga:home",
    };
    let form_title = if editing {
        "Edit server"
    } else {
        "Add a server"
    };
    let servers = model.read().servers.clone();
    rsx! {
        main {
            header {
                class: "f-page-head",
                h1 { class: "f-h1", "Servers" }
                p { class: "f-status", "{status}" }
            }
            section {
                class: "f-card",
                h2 { class: "f-card__title", "{form_title}" }
                div {
                    class: "f-field",
                    label { class: "f-label", r#for: "server-name", "Name" }
                    input {
                        id: "server-name",
                        class: "f-input",
                        value: "{name}",
                        oninput: move |event| model.write().server_name = event.value(),
                    }
                }
                div {
                    class: "f-field",
                    label { class: "f-label", r#for: "server-url", "URL" }
                    input {
                        id: "server-url",
                        class: "f-input",
                        value: "{url}",
                        oninput: move |event| model.write().server_url = event.value(),
                    }
                }
                if !editing {
                    div {
                        class: "f-field",
                        p { class: "f-label", "Kind" }
                        div {
                            class: "f-seg",
                            {kind_button(model, "opds", "OPDS", &kind)}
                            {kind_button(model, "komga", "Komga", &kind)}
                            {kind_button(model, "kavita", "Kavita", &kind)}
                            {kind_button(model, "librarian", "Librarian", &kind)}
                        }
                    }
                }
                div {
                    class: "f-field",
                    label { class: "f-label", r#for: "server-secret", "{secret_heading}" }
                    input {
                        id: "server-secret",
                        class: "f-input",
                        value: "{secret}",
                        placeholder: "{secret_placeholder}",
                        oninput: move |event| model.write().server_secret = event.value(),
                    }
                }
                p { class: "f-status f-status--strong", "{status}" }
                if editing {
                    div {
                        class: "f-actions",
                        if kind == "kavita" {
                            button {
                                class: "f-btn f-btn--action",
                                onclick: move |_| test_edit_clicked(model),
                                "Test connection"
                            }
                        }
                        button {
                            class: "f-btn f-btn--primary f-btn--action",
                            onclick: move |_| save_edit_clicked(model),
                            "Save changes"
                        }
                        button {
                            class: "f-btn f-btn--action",
                            onclick: move |_| cancel_edit_clicked(model),
                            "Cancel"
                        }
                    }
                } else {
                    div {
                        class: "f-actions",
                        button {
                            class: "f-btn f-btn--action",
                            onclick: move |_| test_connection_clicked(model),
                            "Test connection"
                        }
                        button {
                            class: "f-btn f-btn--primary f-btn--action",
                            onclick: move |_| add_server_clicked(model),
                            "Add server"
                        }
                    }
                }
            }
            h2 { class: "f-h2", "Saved servers" }
            if servers.is_empty() {
                p { class: "f-empty", "No servers yet." }
            }
            for server in server_rows(&servers) {
                {server_block(model, server)}
            }
        }
    }
}

fn section_body(model: ModelSignal, action: &str) -> Element {
    match action {
        "appearance" => appearance_section(model),
        "reader" => reader_section(model),
        "library" => library_section(model),
        "sync_now" => sync_section(model),
        _ => rsx! { p { } },
    }
}

fn appearance_section(model: ModelSignal) -> Element {
    let appearance = model.read().appearance.clone();
    rsx! {
        div {
            class: "f-seg f-gap-top",
            for (value, label) in APPEARANCES {
                button {
                    key: "{value}",
                    class: "f-seg__item",
                    aria_pressed: pressed(appearance == value),
                    onclick: move |_| set_appearance(model, value.to_owned()),
                    "{label}"
                }
            }
        }
        p { class: "f-help", "System is shown as light on this reader." }
    }
}

fn reader_section(model: ModelSignal) -> Element {
    let font_px = model.read().font_px;
    rsx! {
        div {
            class: "f-stepper f-gap-top",
            button { class: "f-btn", onclick: move |_| change_font(model, false), "A-" }
            span { class: "f-stepper__value", "Font {font_px} px" }
            button { class: "f-btn", onclick: move |_| change_font(model, true), "A+" }
        }
    }
}

fn library_section(model: ModelSignal) -> Element {
    rsx! {
        div {
            class: "f-actions",
            button {
                class: "f-btn f-btn--action",
                onclick: move |_| start_scan(model),
                "Rescan"
            }
        }
    }
}

fn sync_section(model: ModelSignal) -> Element {
    rsx! {
        div {
            class: "f-actions",
            button {
                class: "f-btn f-btn--action",
                onclick: move |_| sync_clicked(model),
                "Sync now"
            }
        }
    }
}

fn section_title(action: &str) -> &'static str {
    match action {
        "appearance" => "Appearance",
        "reader" => "Reader",
        "library" => "Library",
        "sync_now" => "Sync now",
        _ => "Settings",
    }
}

fn kind_button(
    mut model: ModelSignal,
    id: &'static str,
    label: &'static str,
    current: &str,
) -> Element {
    rsx! {
        button {
            key: "{id}",
            class: "f-seg__item",
            aria_pressed: pressed(current == id),
            onclick: move |_| model.write().server_kind = id.to_owned(),
            "{label}"
        }
    }
}

fn server_block(model: ModelSignal, server: ServerRow) -> Element {
    let browse_id = server.id.clone();
    let edit_id = server.id.clone();
    let adopt_id = server.id.clone();
    let test_id = server.id.clone();
    let delete_id = server.id.clone();
    let raw = server.raw_opds;
    let kavita = server.kavita;
    rsx! {
        article {
            key: "{server.id}",
            class: "f-card",
            div {
                class: "f-card__head",
                h3 { class: "f-card__title", "{server.name}" }
                span { class: "f-badge", "{server.kind}" }
            }
            p { class: "f-line", "{server.url}" }
            p { class: "f-help", "{server.secret}" }
            div {
                class: "f-actions",
                button {
                    class: "f-btn f-btn--action",
                    onclick: move |_| browse_clicked(model, browse_id.clone()),
                    "Browse"
                }
                button {
                    class: "f-btn f-btn--action",
                    onclick: move |_| edit_server_clicked(model, edit_id.clone()),
                    "Edit"
                }
                if kavita {
                    button {
                        class: "f-btn f-btn--action",
                        onclick: move |_| test_saved_clicked(model, test_id.clone()),
                        "Test connection"
                    }
                }
                if raw {
                    button {
                        class: "f-btn f-btn--action",
                        onclick: move |_| use_as_kavita_clicked(model, adopt_id.clone()),
                        "Use as Kavita"
                    }
                }
                button {
                    class: "f-btn f-btn--danger f-btn--action",
                    onclick: move |_| delete_server_clicked(model, delete_id.clone()),
                    "Delete"
                }
            }
        }
    }
}

struct ServerRow {
    id: String,
    name: String,
    kind: String,
    url: String,
    secret: String,
    raw_opds: bool,
    kavita: bool,
}

fn server_rows(servers: &[folio_storage::StoredServer]) -> Vec<ServerRow> {
    servers
        .iter()
        .map(|server| ServerRow {
            id: server.id.clone(),
            name: server.display_name.clone(),
            kind: server.kind.clone(),
            url: server.url.clone(),
            secret: secret_label(server.secret_ref.as_deref()),
            raw_opds: adoptable_opds(&server.kind, server.secret_ref.as_deref()),
            kavita: server.kind == "kavita",
        })
        .collect()
}

fn secret_label(secret: Option<&str>) -> String {
    match secret {
        Some(secret) if secret.starts_with("secret:kavita:") => "Auth key stored".to_owned(),
        Some(secret) if secret.starts_with("keyring:") || secret.starts_with("secret:") => {
            format!("Secret key: {secret}")
        }
        Some(secret) if !secret.is_empty() => "Secret stored".to_owned(),
        _ => "No secret key".to_owned(),
    }
}
