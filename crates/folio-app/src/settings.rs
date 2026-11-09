//! Settings tab. Appearance follows the system until the reader overrides it.

/// Theme choices on the settings screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    /// Follow the system.
    System,
    /// Light.
    Light,
    /// Dark.
    Dark,
    /// Black background.
    Amoled,
}

/// The default appearance.
pub fn default_appearance() -> Appearance {
    Appearance::System
}

/// Settings groups. There is no Test button.
///
/// `sync_now` is present because phase 17 replay can succeed for Komga.
pub fn settings_actions() -> &'static [&'static str] {
    &["appearance", "reader", "library", "sync_now"]
}

#[cfg(test)]
mod tests {
    use super::{default_appearance, settings_actions, Appearance};
    use folio_server::{AuthConfig, ServerConnection, ServerKind};
    use folio_storage::{
        export_backup, flag_or_off, import_backup, is_copied_file, upsert_server, write_log,
        CatalogItem, Database, StoredServer,
    };

    #[test]
    fn settings_round_trip_backup_without_a_test_button() {
        assert_eq!(default_appearance(), Appearance::System);
        assert_eq!(
            settings_actions(),
            ["appearance", "reader", "library", "sync_now"]
        );
        assert!(!settings_actions().contains(&"test_connection"));
        assert!(settings_actions().contains(&"sync_now"));

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("folio.sqlite");
        let json = {
            let db = Database::open(&path).expect("open");
            db.set_setting("appearance", "dark").expect("theme");
            db.set_setting("reader.epub.font", "serif").expect("reader");
            db.set_setting("cache.pages", "20").expect("cache");
            db.set_setting("reader.css", "body{}").expect("css");
            db.set_setting(
                "tree_grant",
                "content://com.android.externalstorage.documents/tree/primary",
            )
            .expect("grant");
            db.set_progress("book-1", "epubcfi(/6/2)")
                .expect("progress");
            db.upsert_item(sample("book-1")).expect("item");
            upsert_server(
                &db,
                StoredServer {
                    id: "local".to_owned(),
                    kind: "librarian".to_owned(),
                    url: "http://127.0.0.1:9".to_owned(),
                    display_name: "Local".to_owned(),
                    secret_ref: Some("keyring:librarian:local".to_owned()),
                },
            )
            .expect("server");
            let json = export_backup(&db).expect("export");
            assert!(!json.contains("password"));
            assert!(!json.contains("librarian_session="));
            assert!(json.contains("keyring:librarian:local"));
            assert!(!is_copied_file(
                "content://com.android.externalstorage.documents/tree/primary"
            ));
            json
        };
        let fresh = dir.path().join("fresh.sqlite");
        let db = Database::open(&fresh).expect("fresh");
        import_backup(&db, &json).expect("import");
        let settings = db.settings().expect("settings");
        assert_eq!(flag_or_off(&settings, "appearance"), "dark");
        assert_eq!(flag_or_off(&settings, "reader.epub.font"), "serif");
        assert_eq!(flag_or_off(&settings, "cache.pages"), "20");
        assert_eq!(flag_or_off(&settings, "reader.css"), "body{}");
        assert_eq!(flag_or_off(&settings, "analytics"), "off");
        assert_eq!(flag_or_off(&settings, "crash_reporting"), "off");
        assert!(db.progress("book-1").expect("progress").is_some());
        assert_eq!(db.item_ids().expect("items"), ["book-1"]);
        let servers = folio_storage::servers(&db).expect("servers");
        assert_eq!(
            servers[0].secret_ref.as_deref(),
            Some("keyring:librarian:local")
        );
        db.set_setting("keep", "yes").expect("canary");
        assert!(import_backup(&db, "{\"schema\":1}").is_err());
        assert_eq!(flag_or_off(&db.settings().expect("kept"), "keep"), "yes");
        let log = dir.path().join("folio.log");
        write_log(&log, "folio log").expect("log");
        assert_eq!(std::fs::read_to_string(&log).expect("read"), "folio log");

        let connection = ServerConnection::new(
            ServerKind::Librarian,
            "http://127.0.0.1:9",
            "Local",
            AuthConfig::LibrarianSession {
                secret_ref: "keyring:librarian:local".into(),
            },
        );
        assert!(connection.browse().is_err());
    }

    fn sample(id: &str) -> CatalogItem {
        CatalogItem {
            id: id.to_owned(),
            source: id.to_owned(),
            label: id.to_owned(),
            file_type: "epub".to_owned(),
            size: None,
            modified_ms: None,
            series_guess: String::new(),
            series: None,
            title: id.to_owned(),
            page_count: None,
            image_child_count: None,
            fingerprint: String::new(),
            enriched_fingerprint: None,
        }
    }
}
