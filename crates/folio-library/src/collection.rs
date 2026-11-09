//! Named collections, one smart rule, tags, ratings, and history.
//!
//! Sync of these rows stays unimplemented. Recently Read is not this list.

use folio_storage::{history, smart_rule, Database, StorageError};

/// A book a smart rule can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartItem {
    /// Book id.
    pub id: String,
    /// File type such as `epub`.
    pub file_type: String,
    /// Tags.
    pub tags: Vec<String>,
    /// Half-stars, 1..=10.
    pub rating_halves: Option<u8>,
}

/// One filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartRule {
    /// `file_type` equals this value.
    FileType(String),
    /// The item has this tag.
    Tag(String),
    /// Rating is at least this many half-stars.
    RatingAtLeast(u8),
}

/// Whether `item` matches `rule`.
pub fn smart_match(item: &SmartItem, rule: &SmartRule) -> bool {
    match rule {
        SmartRule::FileType(file_type) => item.file_type == *file_type,
        SmartRule::Tag(tag) => item.tags.iter().any(|stored| stored == tag),
        SmartRule::RatingAtLeast(halves) => {
            item.rating_halves.is_some_and(|stored| stored >= *halves)
        }
    }
}

/// Load a stored rule and keep the items it matches.
///
/// # Errors
///
/// Returns an error when the rule cannot be read.
pub fn smart_members<'a>(
    db: &Database,
    collection_id: &str,
    items: &'a [SmartItem],
) -> Result<Vec<&'a SmartItem>, StorageError> {
    let Some((field, expected)) = smart_rule(db, collection_id)? else {
        return Ok(Vec::new());
    };
    let rule = match field.as_str() {
        "file_type" => SmartRule::FileType(expected),
        "tag" => SmartRule::Tag(expected),
        "rating" => {
            let halves: u8 = expected.parse().map_err(|_| StorageError::Rating)?;
            SmartRule::RatingAtLeast(halves)
        }
        _ => return Err(StorageError::Rating),
    };
    Ok(items
        .iter()
        .filter(|item| smart_match(item, &rule))
        .collect())
}

/// History book ids, newest first. This is not the library's Recently Read shelf.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn history_ids(db: &Database) -> Result<Vec<String>, StorageError> {
    Ok(history(db)?.into_iter().map(|(id, _)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::{history_ids, smart_match, smart_members, SmartItem, SmartRule};
    use folio_storage::{
        add_collection_item, add_tag, clear_history, clear_history_one, collection_items,
        create_collection, open_history, rating, remove_collection_item, set_rating,
        set_smart_rule, tags, Database,
    };

    #[test]
    fn collection_tags_ratings_and_history() {
        let epub = SmartItem {
            id: "epub".to_owned(),
            file_type: "epub".to_owned(),
            tags: vec!["cave".to_owned()],
            rating_halves: Some(7),
        };
        let cbz = SmartItem {
            id: "cbz".to_owned(),
            file_type: "cbz".to_owned(),
            tags: Vec::new(),
            rating_halves: Some(2),
        };
        assert!(smart_match(&epub, &SmartRule::FileType("epub".to_owned())));
        assert!(!smart_match(&cbz, &SmartRule::Tag("cave".to_owned())));

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("folio.sqlite");
        {
            let db = Database::open(&path).expect("open");
            create_collection(&db, "fav", "Favourites").expect("create");
            add_collection_item(&db, "fav", "epub").expect("add");
            add_collection_item(&db, "fav", "cbz").expect("add");
            remove_collection_item(&db, "fav", "cbz").expect("remove");
            add_tag(&db, "epub", "cave").expect("tag");
            set_rating(&db, "epub", 7).expect("rating");
            set_smart_rule(&db, "smart", "file_type", "epub").expect("rule");
            open_history(&db, "older", 1).expect("history");
            open_history(&db, "newer", 2).expect("history");
        }
        let db = Database::open(&path).expect("reopen");
        assert_eq!(collection_items(&db, "fav").expect("items"), ["epub"]);
        assert_eq!(tags(&db, "epub").expect("tags"), ["cave"]);
        assert_eq!(rating(&db, "epub").expect("rating"), Some(7));
        let books = [epub, cbz];
        let members = smart_members(&db, "smart", &books).expect("smart");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id, "epub");
        assert_eq!(history_ids(&db).expect("history"), ["newer", "older"]);
        clear_history_one(&db, "newer").expect("clear one");
        assert_eq!(history_ids(&db).expect("history"), ["older"]);
        clear_history(&db).expect("clear all");
        assert!(history_ids(&db).expect("empty").is_empty());
    }
}
