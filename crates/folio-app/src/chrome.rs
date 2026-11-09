//! Chrome stylesheet and theme selection. Book pages keep `folio_reader::theme_css`.

/// The chrome stylesheet: four token blocks, then component classes.
pub(crate) const CHROME_CSS: &str = include_str!("../../../assets/chrome.css");

/// `data-theme` value for an appearance or paper setting. `system` and unknown names are light.
pub(crate) fn theme_attr(name: &str) -> &'static str {
    match name {
        "sepia" => "sepia",
        "dark" => "dark",
        "amoled" => "amoled",
        _ => "light",
    }
}

/// The Quire mark on Android's 108-unit adaptive-icon grid. The brand SVGs and the
/// launcher vector carry these same strings; a test keeps them in step.
pub(crate) const MARK_PATHS: [&str; 3] = [
    "M30 46C36 38 48 38 54 46C60 38 72 38 78 46",
    "M30 58C36 50 48 50 54 58C60 50 72 50 78 58",
    "M30 70C36 62 48 62 54 70C60 62 72 62 78 70",
];

#[cfg(test)]
mod tests {
    use super::{theme_attr, CHROME_CSS};

    const COLOUR_TOKENS: [&str; 11] = [
        "--paper",
        "--ink",
        "--ink-2",
        "--muted",
        "--rule",
        "--card",
        "--card-border",
        "--field-border",
        "--track",
        "--accent",
        "--danger",
    ];

    fn theme_block(theme: &str) -> &'static str {
        let opener = format!("[data-theme=\"{theme}\"]{{");
        let start = CHROME_CSS
            .find(&opener)
            .unwrap_or_else(|| panic!("no block for {theme}"))
            + opener.len();
        let rest = &CHROME_CSS[start..];
        let end = rest.find('}').expect("block closes");
        &rest[..end]
    }

    const UI_SOURCES: [(&str, &str); 6] = [
        ("ui/mod.rs", include_str!("ui/mod.rs")),
        ("ui/library.rs", include_str!("ui/library.rs")),
        ("ui/places.rs", include_str!("ui/places.rs")),
        ("ui/kavita.rs", include_str!("ui/kavita.rs")),
        ("ui/reading.rs", include_str!("ui/reading.rs")),
        ("ui/icons.rs", include_str!("ui/icons.rs")),
    ];

    const COMPONENT_CLASSES: [&str; 62] = [
        "f-app",
        "f-h1",
        "f-h2",
        "f-h2--flush",
        "f-status",
        "f-status--strong",
        "f-meta",
        "f-meta-strong",
        "f-help",
        "f-empty",
        "f-line",
        "f-gap-top",
        "f-page-head",
        "f-header",
        "f-header-row",
        "f-brand",
        "f-wordmark",
        "f-mark",
        "f-icon",
        "f-btn",
        "f-btn--primary",
        "f-btn--danger",
        "f-btn--action",
        "f-btn--block",
        "f-actions",
        "f-stepper",
        "f-stepper__value",
        "f-seg",
        "f-seg__item",
        "f-field",
        "f-label",
        "f-input",
        "f-search",
        "f-search__icon",
        "f-card",
        "f-card__head",
        "f-card__title",
        "f-badge",
        "f-grid",
        "f-row",
        "f-book",
        "f-book__title",
        "f-cover",
        "f-cover__fallback",
        "f-continue",
        "f-continue__body",
        "f-progress",
        "f-progress__fill",
        "f-chips",
        "f-chip",
        "f-tiles",
        "f-tile",
        "f-tile__value",
        "f-tile__label",
        "f-list",
        "f-list__item",
        "f-list__item--split",
        "f-list__item--actions",
        "f-list__text",
        "f-list__buttons",
        "f-detail-head",
        "f-stack",
    ];

    const MENU_AND_NAV_CLASSES: [&str; 11] = [
        "f-nav",
        "f-tab",
        "f-tab__icon",
        "f-menu",
        "f-menu--top",
        "f-menu--bottom",
        "f-menu__title",
        "f-menu__progress",
        "f-menu__group",
        "f-menu__label",
        "f-menu__buttons",
    ];

    fn css_has_class(name: &str) -> bool {
        let needle = format!(".{name}");
        CHROME_CSS.match_indices(&needle).any(|(index, _)| {
            let next = CHROME_CSS.as_bytes().get(index + needle.len()).copied();
            !matches!(next, Some(byte) if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
    }

    fn used_classes(source: &str) -> Vec<&str> {
        source
            .match_indices("class: \"")
            .filter_map(|(index, opener)| {
                let rest = &source[index + opener.len()..];
                rest.find('"').map(|end| &rest[..end])
            })
            .flat_map(str::split_whitespace)
            .filter(|name| name.starts_with("f-") && !name.contains('{'))
            .collect()
    }

    fn colour_literals(source: &str) -> Vec<String> {
        let bytes = source.as_bytes();
        let mut found = Vec::new();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte != b'#' {
                continue;
            }
            let digits = bytes[index + 1..]
                .iter()
                .take_while(|next| next.is_ascii_hexdigit())
                .count();
            let after = bytes.get(index + 1 + digits).copied().unwrap_or(b' ');
            if matches!(digits, 3 | 6 | 8) && !after.is_ascii_alphanumeric() {
                found.push(source[index..index + 1 + digits].to_owned());
            }
        }
        for function in ["rgb(", "rgba(", "hsl("] {
            if source.contains(function) {
                found.push(function.to_owned());
            }
        }
        found
    }

    fn assert_no_colours(name: &str) {
        let source = UI_SOURCES
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, source)| *source)
            .unwrap_or_else(|| panic!("{name} is not in UI_SOURCES"));
        let found = colour_literals(source);
        assert!(found.is_empty(), "{name} hardcodes colours: {found:?}");
    }

    /// Lines inside an `rsx!` block that read the model. A guard made inside `rsx!` lives until
    /// the whole node is built, so a function called from the same block that reads the model
    /// again deadlocks once a background thread is waiting to write.
    fn model_reads_inside_rsx(source: &str) -> Vec<usize> {
        let mut found = Vec::new();
        let mut depth = 0usize;
        let mut open_rsx: Vec<usize> = Vec::new();
        let mut pending = false;
        for (index, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let mut chars = line.char_indices();
            while let Some((at, ch)) = chars.next() {
                if line[at..].starts_with("rsx!") {
                    pending = true;
                    chars.nth(2);
                    continue;
                }
                match ch {
                    '{' => {
                        depth += 1;
                        if pending {
                            open_rsx.push(depth);
                            pending = false;
                        }
                    }
                    '}' => {
                        if open_rsx.last() == Some(&depth) {
                            open_rsx.pop();
                        }
                        depth = depth.saturating_sub(1);
                    }
                    _ => {}
                }
            }
            if !open_rsx.is_empty()
                && (line.contains("model.read()") || line.contains("model.peek()"))
            {
                found.push(index + 1);
            }
        }
        found
    }

    #[test]
    fn rsx_scan_finds_a_read_inside_a_block_but_not_before_it() {
        let source =
            "fn a(model: M) -> Element {\n    let x = model.read().y;\n    rsx! {\n        \
                      {f(model, model.read().z)}\n    }\n}\nfn b(model: M) -> Element {\n    \
                      // a comment may say rsx! without opening one\n    let z = {\n        \
                      model.read().w\n    };\n    rsx! { p { \"{z}\" } }\n}\n";
        assert_eq!(model_reads_inside_rsx(source), vec![4]);
    }

    #[test]
    fn ui_never_reads_the_model_inside_rsx() {
        for (name, source) in UI_SOURCES {
            let lines = model_reads_inside_rsx(source);
            assert!(
                lines.is_empty(),
                "{name} reads the model inside rsx! at lines {lines:?}"
            );
        }
    }

    #[test]
    fn component_classes_are_defined() {
        for name in COMPONENT_CLASSES.iter().chain(MENU_AND_NAV_CLASSES.iter()) {
            assert!(css_has_class(name), "chrome.css lacks .{name}");
        }
    }

    #[test]
    fn form_control_reset_does_not_outrank_component_classes() {
        // A class-qualified reset such as `.f-app button` scores (0,1,1) and beats
        // `.f-btn--primary` (0,1,0), which painted the primary button ink on ink.
        assert!(CHROME_CSS.contains("button,input,select{font:inherit;color:inherit}"));
        assert!(!CHROME_CSS.contains(".f-app button"));
    }

    #[test]
    fn every_class_used_in_ui_is_defined() {
        for (file, source) in UI_SOURCES {
            for name in used_classes(source) {
                assert!(
                    css_has_class(name),
                    "{file} uses .{name}, which chrome.css lacks"
                );
            }
        }
    }

    #[test]
    fn colour_literal_scan_finds_hex_and_functions_but_not_attributes() {
        assert_eq!(
            colour_literals("#[cfg(test)] fn a() {}"),
            Vec::<String>::new()
        );
        assert_eq!(colour_literals("background:#111111;"), ["#111111"]);
        assert_eq!(colour_literals("color:#fff;"), ["#fff"]);
        assert_eq!(colour_literals("rgba(255,255,255,0.9)"), ["rgba("]);
    }

    #[test]
    fn reading_rs_has_no_colour_literals() {
        assert_no_colours("ui/reading.rs");
    }

    #[test]
    fn library_rs_has_no_colour_literals() {
        assert_no_colours("ui/library.rs");
    }

    #[test]
    fn places_rs_has_no_colour_literals() {
        assert_no_colours("ui/places.rs");
    }

    #[test]
    fn kavita_rs_has_no_colour_literals() {
        assert_no_colours("ui/kavita.rs");
    }

    #[test]
    fn mod_rs_has_no_colour_literals() {
        assert_no_colours("ui/mod.rs");
    }

    #[test]
    fn icons_rs_has_no_colour_literals() {
        assert_no_colours("ui/icons.rs");
    }

    #[test]
    fn brand_files_and_launcher_draw_the_same_mark() {
        let mark = include_str!("../../../assets/brand/folio-mark.svg");
        let icon = include_str!("../../../assets/brand/folio-icon.svg");
        let launcher =
            include_str!("../../../assets/android/res/drawable-v24/ic_launcher_foreground.xml");
        for path in super::MARK_PATHS {
            assert!(
                mark.contains(&format!("d=\"{path}\"")),
                "folio-mark.svg lacks {path}"
            );
            assert!(
                icon.contains(&format!("d=\"{path}\"")),
                "folio-icon.svg lacks {path}"
            );
            assert!(
                launcher.contains(&format!("android:pathData=\"{path}\"")),
                "launcher vector lacks {path}"
            );
        }
    }

    #[test]
    fn mark_stays_inside_the_launcher_safe_zone() {
        for path in super::MARK_PATHS {
            let numbers = path
                .split(|c: char| !c.is_ascii_digit())
                .filter(|part| !part.is_empty())
                .map(|part| part.parse::<i32>().expect("integer coordinate"))
                .collect::<Vec<_>>();
            assert_eq!(numbers.len() % 2, 0, "{path} has an odd coordinate count");
            for point in numbers.chunks(2) {
                let (dx, dy) = (point[0] - 54, point[1] - 54);
                assert!(
                    dx * dx + dy * dy <= 885,
                    "{point:?} in {path} leaves the safe zone"
                );
            }
        }
    }

    #[test]
    fn every_theme_defines_every_colour_token() {
        for theme in ["light", "sepia", "dark", "amoled"] {
            let block = theme_block(theme);
            for token in COLOUR_TOKENS {
                assert!(
                    block.contains(&format!("{token}:")),
                    "{theme} lacks {token}"
                );
            }
        }
    }

    #[test]
    fn theme_attr_maps_settings_to_stylesheet_themes() {
        for (setting, theme) in [
            ("light", "light"),
            ("sepia", "sepia"),
            ("dark", "dark"),
            ("amoled", "amoled"),
            ("system", "light"),
            ("", "light"),
        ] {
            assert_eq!(theme_attr(setting), theme, "{setting}");
        }
    }
}
