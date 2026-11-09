//! Keyboard shortcuts for a desktop or tablet layout. The phone layout has none.

/// Where the key event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLayout {
    /// Keyboard attached to a desktop or tablet window.
    Desktop,
    /// Phone. Hardware reader keys are not bound.
    Phone,
}

/// Reader command produced by a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderCommand {
    /// Previous page or chapter.
    Previous,
    /// Next page or chapter.
    Next,
    /// Toggle fullscreen.
    Fullscreen,
    /// Toggle a bookmark.
    Bookmark,
}

/// Map `key` on `layout`. Phone keys return [`None`].
pub fn binding(layout: KeyLayout, key: &str) -> Option<ReaderCommand> {
    if layout == KeyLayout::Phone {
        return None;
    }
    match key {
        "Left" => Some(ReaderCommand::Previous),
        "Right" => Some(ReaderCommand::Next),
        "f" => Some(ReaderCommand::Fullscreen),
        "b" => Some(ReaderCommand::Bookmark),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{binding, KeyLayout, ReaderCommand};

    #[test]
    fn desktop_keys_turn_pages_and_phone_keys_do_not() {
        assert_eq!(
            binding(KeyLayout::Desktop, "Left"),
            Some(ReaderCommand::Previous)
        );
        assert_eq!(
            binding(KeyLayout::Desktop, "Right"),
            Some(ReaderCommand::Next)
        );
        assert_eq!(
            binding(KeyLayout::Desktop, "f"),
            Some(ReaderCommand::Fullscreen)
        );
        assert_eq!(
            binding(KeyLayout::Desktop, "b"),
            Some(ReaderCommand::Bookmark)
        );
        assert_eq!(binding(KeyLayout::Desktop, "x"), None);
        for key in ["Left", "Right", "f", "b"] {
            assert_eq!(binding(KeyLayout::Phone, key), None);
        }
    }
}
