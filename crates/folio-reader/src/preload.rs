//! Comic preload. Reversing direction drops the forward window.

/// Pages that were cancelled, and the pages to request instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreloadPlan {
    /// Pages from the previous direction that must not be fetched.
    pub cancelled: Vec<u32>,
    /// Pages to fetch now, in ascending order.
    pub requested: Vec<u32>,
}

/// Up to three pages after `current`.
pub fn forward_window(current: u32, page_count: u32) -> Vec<u32> {
    (1..=3u32)
        .filter_map(|step| {
            let page = current.checked_add(step)?;
            (page < page_count).then_some(page)
        })
        .collect()
}

/// Cancel `forward` and request up to three pages before `current`.
pub fn reverse_preload(forward: &[u32], current: u32, page_count: u32) -> PreloadPlan {
    let mut requested: Vec<u32> = (1..=3u32)
        .filter_map(|step| current.checked_sub(step))
        .filter(|page| *page < page_count)
        .collect();
    requested.sort_unstable();
    PreloadPlan {
        // to_vec: the plan owns the cancelled page list
        cancelled: forward.to_vec(),
        requested,
    }
}

#[cfg(test)]
mod tests {
    use super::{forward_window, reverse_preload};

    #[test]
    fn preload_reverse_cancels_the_forward_window() {
        let forward = forward_window(10, 40);
        assert_eq!(forward, vec![11, 12, 13]);
        let plan = reverse_preload(&forward, 10, 40);
        assert_eq!(plan.cancelled, vec![11, 12, 13]);
        assert_eq!(plan.requested, vec![7, 8, 9]);
        assert!(plan.requested.len() < 40);
    }
}
