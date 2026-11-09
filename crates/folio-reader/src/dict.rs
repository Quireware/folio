//! Dictionary lookup through a caller-supplied list.
//!
//! Offline WordNet is not linked.

/// One definition from the injected client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// The word that matched.
    pub word: String,
    /// The definition text.
    pub text: String,
}

/// Look up `word` in `entries`. Comparison is ASCII case-insensitive.
pub fn lookup(entries: &[(&str, &str)], word: &str) -> Option<Definition> {
    let folded = word.to_ascii_lowercase();
    entries.iter().find_map(|(entry, text)| {
        entry.eq_ignore_ascii_case(&folded).then(|| Definition {
            word: (*entry).to_owned(),
            text: (*text).to_owned(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::lookup;

    #[test]
    fn dict_uses_the_injected_client() {
        let entries = [("folio", "a leaf of a book")];
        let found = lookup(&entries, "Folio").expect("definition");
        assert_eq!(found.text, "a leaf of a book");
        assert!(lookup(&entries, "wordnet").is_none());
    }
}
