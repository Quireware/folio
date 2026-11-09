//! Progress sync queue.
//!
//! Replay sends a winning update only to adapters that can store progress.
//! Plain OPDS and librarian report unsupported. They are not a successful send.
#![deny(missing_docs)]

use thiserror::Error;

/// Why a sync operation did not run.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SyncError {
    /// The book id or the locator is empty.
    #[error("the progress update is missing a book id or locator")]
    Incomplete,
}

/// How two devices disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConflictPolicy {
    /// The write with the later timestamp replaces the earlier one.
    #[default]
    LastWriteWins,
}

/// One queued progress update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncQueueItem {
    book_id: String,
    locator: String,
    updated_at: i64,
}

impl SyncQueueItem {
    /// A progress update for `book_id` at `updated_at`.
    pub fn progress(
        book_id: impl Into<String>,
        locator: impl Into<String>,
        updated_at: i64,
    ) -> Self {
        Self {
            book_id: book_id.into(),
            locator: locator.into(),
            updated_at,
        }
    }

    /// Book this row refers to.
    pub fn book_id(&self) -> &str {
        &self.book_id
    }

    /// Locator, such as `page:4` or an EPUB CFI.
    pub fn locator(&self) -> &str {
        &self.locator
    }

    /// Client timestamp. A larger value wins for the same book.
    pub fn updated_at(&self) -> i64 {
        self.updated_at
    }
}

/// In-memory view of sync preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncState {
    policy: ConflictPolicy,
}

impl SyncState {
    /// Preferences with an explicit conflict policy.
    pub fn new(policy: ConflictPolicy) -> Self {
        Self { policy }
    }

    /// Conflict policy.
    pub fn policy(&self) -> ConflictPolicy {
        self.policy
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new(ConflictPolicy::LastWriteWins)
    }
}

/// What one adapter did with the winning update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayStatus {
    /// The adapter stored the update.
    Sent {
        /// Adapter name.
        server: &'static str,
    },
    /// The adapter has no progress write. This is not success.
    Unsupported {
        /// Adapter name.
        server: &'static str,
    },
    /// The adapter can store progress but is offline. The row stays queued.
    Offline {
        /// Adapter name.
        server: &'static str,
    },
}

/// A server the queue can try. `name` is the delivery identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    name: &'static str,
    online: bool,
    progress_write: bool,
    sent: Vec<SyncQueueItem>,
}

impl Adapter {
    /// An adapter. `progress_write` is false for plain OPDS.
    pub fn new(name: &'static str, online: bool, progress_write: bool) -> Self {
        Self {
            name,
            online,
            progress_write,
            sent: Vec::new(),
        }
    }

    /// Mark the transport up or down.
    pub fn set_online(&mut self, online: bool) {
        self.online = online;
    }

    /// Updates this adapter has accepted.
    pub fn sent(&self) -> &[SyncQueueItem] {
        &self.sent
    }
}

#[derive(Debug)]
struct Row {
    item: SyncQueueItem,
    delivered: Vec<&'static str>,
}

/// Offline progress queue. Last-write-wins per book.
#[derive(Debug, Default)]
pub struct SyncQueue {
    policy: ConflictPolicy,
    rows: Vec<Row>,
}

impl SyncQueue {
    /// An empty queue using last-write-wins.
    pub fn new() -> Self {
        Self {
            policy: ConflictPolicy::LastWriteWins,
            rows: Vec::new(),
        }
    }

    /// Conflict policy this queue applies.
    pub fn policy(&self) -> ConflictPolicy {
        self.policy
    }

    /// Number of books still waiting.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether nothing is waiting.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Store `item`, or replace the same book when the timestamp is newer or equal.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::Incomplete`] when the book id or locator is empty.
    pub fn enqueue(&mut self, item: SyncQueueItem) -> Result<(), SyncError> {
        if item.book_id.is_empty() || item.locator.is_empty() {
            return Err(SyncError::Incomplete);
        }
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|row| row.item.book_id == item.book_id)
        {
            if item.updated_at >= row.item.updated_at {
                let changed =
                    item.updated_at > row.item.updated_at || item.locator != row.item.locator;
                row.item = item;
                if changed {
                    row.delivered.clear();
                }
            }
            return Ok(());
        }
        self.rows.push(Row {
            item,
            delivered: Vec::new(),
        });
        Ok(())
    }

    /// Send each winning row to adapters that can store it and are online.
    ///
    /// An adapter without a progress write is [`ReplayStatus::Unsupported`].
    /// A capable adapter that is offline keeps the row queued. A row is dropped only when
    /// no capable adapter is still offline. Adapters that already accepted the current
    /// locator are not sent a second copy.
    #[must_use]
    pub fn replay(&mut self, adapters: &mut [Adapter]) -> Vec<ReplayStatus> {
        let mut statuses = Vec::new();
        self.rows.retain_mut(|row| {
            let mut waiting = false;
            for adapter in adapters.iter_mut() {
                if !adapter.progress_write {
                    statuses.push(ReplayStatus::Unsupported {
                        server: adapter.name,
                    });
                    continue;
                }
                if row.delivered.contains(&adapter.name) {
                    continue;
                }
                if !adapter.online {
                    statuses.push(ReplayStatus::Offline {
                        server: adapter.name,
                    });
                    waiting = true;
                    continue;
                }
                // clone: the adapter keeps its own copy after the queue drops the row
                adapter.sent.push(row.item.clone());
                row.delivered.push(adapter.name);
                statuses.push(ReplayStatus::Sent {
                    server: adapter.name,
                });
            }
            waiting
        });
        statuses
    }
}

#[cfg(test)]
mod tests {
    use super::{Adapter, ConflictPolicy, ReplayStatus, SyncError, SyncQueue, SyncQueueItem};
    use folio_server::{deliver_progress, ProgressFixture, ServerKind};

    #[test]
    fn offline_replay_sends_one_update_when_the_transport_returns() {
        let mut queue = SyncQueue::new();
        assert_eq!(queue.policy(), ConflictPolicy::LastWriteWins);
        assert_eq!(
            queue.enqueue(SyncQueueItem::progress("", "page:1", 1)),
            Err(SyncError::Incomplete)
        );
        queue
            .enqueue(SyncQueueItem::progress("book", "page:1", 1))
            .expect("enqueue");
        let mut adapters = [
            Adapter::new("komga", false, true),
            Adapter::new("other", false, true),
        ];
        let waiting = queue.replay(&mut adapters);
        assert_eq!(
            waiting,
            [
                ReplayStatus::Offline { server: "komga" },
                ReplayStatus::Offline { server: "other" }
            ]
        );
        assert_eq!(queue.len(), 1);
        assert!(adapters[0].sent().is_empty());

        adapters[0].set_online(true);
        let once = queue.replay(&mut adapters);
        assert_eq!(
            once,
            [
                ReplayStatus::Sent { server: "komga" },
                ReplayStatus::Offline { server: "other" }
            ]
        );
        assert_eq!(queue.len(), 1);
        assert_eq!(adapters[0].sent().len(), 1);

        let again = queue.replay(&mut adapters);
        assert_eq!(again, [ReplayStatus::Offline { server: "other" }]);
        assert_eq!(adapters[0].sent().len(), 1);

        adapters[1].set_online(true);
        let done = queue.replay(&mut adapters);
        assert_eq!(done, [ReplayStatus::Sent { server: "other" }]);
        assert!(queue.is_empty());
        assert_eq!(adapters[1].sent().len(), 1);
        assert_eq!(adapters[0].sent()[0].locator(), "page:1");
    }

    #[test]
    fn later_timestamp_wins_for_the_same_book() {
        let mut queue = SyncQueue::new();
        queue
            .enqueue(SyncQueueItem::progress("book", "page:1", 1))
            .expect("early");
        queue
            .enqueue(SyncQueueItem::progress("book", "page:9", 5))
            .expect("late");
        queue
            .enqueue(SyncQueueItem::progress("book", "page:2", 3))
            .expect("older than the winner");
        let mut adapters = [Adapter::new("komga", true, true)];
        let status = queue.replay(&mut adapters);
        assert_eq!(status, [ReplayStatus::Sent { server: "komga" }]);
        assert_eq!(adapters[0].sent().len(), 1);
        assert_eq!(adapters[0].sent()[0].locator(), "page:9");
        assert_eq!(adapters[0].sent()[0].updated_at(), 5);
        assert!(queue.is_empty());
    }

    #[test]
    fn opds_and_librarian_are_unsupported_and_komga_stores_progress() {
        let mut queue = SyncQueue::new();
        queue
            .enqueue(SyncQueueItem::progress("book", "page:4", 8))
            .expect("enqueue");
        let mut adapters = [
            Adapter::new("opds", true, false),
            Adapter::new("komga", true, true),
            Adapter::new("librarian", true, false),
        ];
        let status = queue.replay(&mut adapters);
        assert_eq!(
            status,
            [
                ReplayStatus::Unsupported { server: "opds" },
                ReplayStatus::Sent { server: "komga" },
                ReplayStatus::Unsupported {
                    server: "librarian"
                }
            ]
        );
        assert!(adapters[0].sent().is_empty());
        assert!(adapters[2].sent().is_empty());
        assert!(queue.is_empty());
        let sent = &adapters[1].sent()[0];
        let mut fixture = ProgressFixture::default();
        deliver_progress(
            ServerKind::Komga,
            &mut fixture,
            sent.book_id(),
            sent.locator(),
        )
        .expect("komga write");
        assert_eq!(fixture.page("book"), Some(4));
        assert_eq!(
            deliver_progress(
                ServerKind::OpdsV2,
                &mut fixture,
                sent.book_id(),
                sent.locator()
            ),
            Err(folio_server::ServerError::Unsupported)
        );
        assert_eq!(
            deliver_progress(
                ServerKind::Librarian,
                &mut fixture,
                sent.book_id(),
                sent.locator()
            ),
            Err(folio_server::ServerError::Unsupported)
        );
    }
}
