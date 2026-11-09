//! Line icons for the chrome, and the Quire mark.

use dioxus::prelude::*;

use crate::chrome::MARK_PATHS;

/// Icon for a chrome route name from `shell::chrome_labels`.
pub(super) fn nav_icon(name: &str) -> Element {
    match name {
        "statistics" => rsx! {
            svg { class: "f-icon", view_box: "0 0 24 24", "aria-hidden": "true",
                path { d: "M4 20h16M7 16v-5M12 16V6M17 16v-8" }
            }
        },
        "settings" => rsx! {
            svg { class: "f-icon", view_box: "0 0 24 24", "aria-hidden": "true",
                path { d: "M4 7h10M18 7h2M4 17h4M12 17h8" }
                circle { cx: "16", cy: "7", r: "2" }
                circle { cx: "10", cy: "17", r: "2" }
            }
        },
        "history" => rsx! {
            svg { class: "f-icon", view_box: "0 0 24 24", "aria-hidden": "true",
                path { d: "M3.5 12a8.5 8.5 0 1 0 2.6-6.1L3.5 8.5" }
                path { d: "M3.5 3.5v5h5" }
                path { d: "M12 7.5V12l3 2" }
            }
        },
        "servers" => rsx! {
            svg { class: "f-icon", view_box: "0 0 24 24", "aria-hidden": "true",
                rect { x: "3.5", y: "4", width: "17", height: "7", rx: "2" }
                rect { x: "3.5", y: "13", width: "17", height: "7", rx: "2" }
                path { d: "M7.5 7.5h.01M7.5 16.5h.01" }
            }
        },
        _ => rsx! {
            svg { class: "f-icon", view_box: "0 0 24 24", "aria-hidden": "true",
                path { d: "M5 4h3v16H5zM10 4h3v16h-3zM15.4 5.3l2.9-.8 3.6 14.5-2.9.8z" }
            }
        },
    }
}

/// Magnifier inside the search field.
pub(super) fn search_icon() -> Element {
    rsx! {
        svg { class: "f-icon f-search__icon", view_box: "0 0 24 24", "aria-hidden": "true",
            circle { cx: "11", cy: "11", r: "7" }
            path { d: "M20 20l-4-4" }
        }
    }
}

/// The Quire mark, stroked in the current ink colour.
pub(super) fn quire_mark() -> Element {
    rsx! {
        svg { class: "f-mark", view_box: "0 0 108 108", "aria-hidden": "true",
            for d in MARK_PATHS {
                path { key: "{d}", d: "{d}" }
            }
        }
    }
}
