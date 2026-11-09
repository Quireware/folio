//! Continue Reading widget data and launcher shortcuts.

/// What the Continue Reading and streak widgets show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetSnapshot {
    continue_reading: Option<String>,
    streak_days: u32,
}

impl WidgetSnapshot {
    /// `continue_reading` is the last open book id. An empty id is treated as none.
    pub fn new(continue_reading: Option<&str>, streak_days: u32) -> Self {
        let continue_reading = continue_reading
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        Self {
            continue_reading,
            streak_days,
        }
    }

    /// Book id the Continue Reading widget would open.
    pub fn continue_reading(&self) -> Option<&str> {
        self.continue_reading.as_deref()
    }

    /// Streak length from the statistics summary.
    pub fn streak_days(&self) -> u32 {
        self.streak_days
    }
}

/// Launcher shortcuts. Servers is omitted until a server phase has landed.
pub fn shortcuts(servers_landed: bool) -> &'static [&'static str] {
    if servers_landed {
        &["continue-reading", "library", "servers"]
    } else {
        &["continue-reading", "library"]
    }
}

#[cfg(test)]
mod tests {
    use super::{shortcuts, WidgetSnapshot};

    #[test]
    fn widget_continue_reading_and_streak() {
        let widget = WidgetSnapshot::new(Some("dune"), 3);
        assert_eq!(widget.continue_reading(), Some("dune"));
        assert_eq!(widget.streak_days(), 3);
        assert_eq!(WidgetSnapshot::new(Some(""), 0).continue_reading(), None);
    }

    #[test]
    fn shortcuts_include_servers_only_after_that_phase() {
        assert_eq!(shortcuts(true), ["continue-reading", "library", "servers"]);
        assert_eq!(shortcuts(false), ["continue-reading", "library"]);
    }
}
