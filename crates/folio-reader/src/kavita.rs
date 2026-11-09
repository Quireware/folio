//! On-demand Kavita pages. Only the current page and the next page are held.

/// One fetched page. The bytes are not a catalog row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemotePage {
    /// Kavita EPUB HTML for a single page.
    Html(String),
    /// One image or archive page.
    Image(Vec<u8>),
    /// Chapter PDF, held only while the chapter is open.
    Pdf(Vec<u8>),
}

/// A slot in the two-page window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeldPage {
    /// Nothing has been requested.
    Missing,
    /// A fetch is in progress.
    Loading,
    /// The page is ready to draw.
    Ready(RemotePage),
    /// The last fetch failed.
    Failed,
}

/// Current page and the following page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldWindow {
    current: HeldPage,
    next: HeldPage,
}

impl HeldWindow {
    /// Both slots empty.
    pub fn empty() -> Self {
        Self {
            current: HeldPage::Missing,
            next: HeldPage::Missing,
        }
    }

    /// The page on screen.
    pub fn current(&self) -> &HeldPage {
        &self.current
    }

    /// The page after the one on screen.
    pub fn next(&self) -> &HeldPage {
        &self.next
    }

    /// Mark the current slot as loading and drop the following page.
    pub fn loading_current(&mut self) {
        self.current = HeldPage::Loading;
        self.next = HeldPage::Missing;
    }

    /// Store the page on screen.
    pub fn set_current(&mut self, page: RemotePage) {
        self.current = HeldPage::Ready(page);
    }

    /// Store the following page.
    pub fn set_next(&mut self, page: RemotePage) {
        self.next = HeldPage::Ready(page);
    }

    /// The current fetch failed.
    pub fn fail_current(&mut self) {
        self.current = HeldPage::Failed;
    }

    /// The following page becomes current. The next slot is empty.
    pub fn shift_forward(&mut self) {
        self.current = std::mem::replace(&mut self.next, HeldPage::Missing);
    }
}

/// Which page index is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageWindow {
    index: i64,
    count: i64,
}

impl PageWindow {
    /// Open at the first page. A non-positive count stays at page 0.
    pub fn new(count: i64) -> Self {
        Self {
            index: 0,
            count: count.max(0),
        }
    }

    /// Open at `index` when it is inside the chapter.
    pub fn at(count: i64, index: i64) -> Self {
        let count = count.max(0);
        let index = if count == 0 {
            0
        } else {
            index.clamp(0, count.saturating_sub(1))
        };
        Self { index, count }
    }

    /// Page on screen.
    pub fn index(self) -> i64 {
        self.index
    }

    /// Pages in the chapter.
    pub fn count(self) -> i64 {
        self.count
    }

    /// The page after this one, when the chapter has one.
    pub fn next_index(self) -> Option<i64> {
        let next = self.index.saturating_add(1);
        (next < self.count).then_some(next)
    }

    /// Move by `delta` pages. `false` means the index did not change.
    pub fn turn(&mut self, delta: i64) -> bool {
        if self.count == 0 {
            return false;
        }
        let last = self.count.saturating_sub(1);
        let next = self.index.saturating_add(delta).clamp(0, last);
        if next == self.index {
            return false;
        }
        self.index = next;
        true
    }
}

/// A chapter can open when it is fully downloaded or the server can be reached.
pub fn can_open(downloaded: bool, online: bool) -> bool {
    downloaded || online
}

/// One stored page and the images that page references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredPage {
    /// Zero-based page index.
    pub index: i64,
    /// Images the page HTML or archive entry names.
    pub images_expected: u64,
    /// Images written beside that page.
    pub images_stored: u64,
}

/// A download is complete only when every page and that page's images are stored.
pub fn download_complete(page_count: i64, stored: &[StoredPage]) -> bool {
    if page_count <= 0 {
        return false;
    }
    let expected = u64::try_from(page_count).unwrap_or(0);
    if u64::try_from(stored.len()).unwrap_or(0) != expected {
        return false;
    }
    for index in 0..page_count {
        let Some(page) = stored.iter().find(|page| page.index == index) else {
            return false;
        };
        if page.images_stored != page.images_expected {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{can_open, download_complete, HeldWindow, PageWindow, RemotePage, StoredPage};

    #[test]
    fn the_window_keeps_only_the_current_page_and_the_next() {
        let mut window = PageWindow::new(4);
        assert_eq!(window.next_index(), Some(1));
        assert!(window.turn(1));
        assert_eq!(window.index(), 1);
        let mut held = HeldWindow::empty();
        held.set_current(RemotePage::Html("<p>1</p>".to_owned()));
        held.set_next(RemotePage::Html("<p>2</p>".to_owned()));
        held.shift_forward();
        match held.current() {
            super::HeldPage::Ready(RemotePage::Html(html)) => assert_eq!(html, "<p>2</p>"),
            other => panic!("expected the next page, got {other:?}"),
        }
        assert!(matches!(held.next(), super::HeldPage::Missing));
    }

    #[test]
    fn offline_without_a_download_does_not_open() {
        assert!(!can_open(false, false));
        assert!(can_open(true, false));
        assert!(can_open(false, true));
    }

    #[test]
    fn a_download_is_complete_only_when_every_page_and_image_is_stored() {
        let pages = [
            StoredPage {
                index: 0,
                images_expected: 1,
                images_stored: 1,
            },
            StoredPage {
                index: 1,
                images_expected: 0,
                images_stored: 0,
            },
        ];
        assert!(download_complete(2, &pages));
        let missing_image = [StoredPage {
            index: 0,
            images_expected: 2,
            images_stored: 1,
        }];
        assert!(!download_complete(1, &missing_image));
        assert!(!download_complete(2, &pages[..1]));
    }
}
