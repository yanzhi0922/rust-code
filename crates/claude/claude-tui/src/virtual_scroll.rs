//! Virtual scroll engine for large item lists.
//!
//! Provides efficient rendering of large lists by only computing
//! visible ranges. Inspired by Claude Code's virtual scroll hook.
//! Supports:
//! - Dynamic item heights with estimation
//! - Overscan rendering for smooth scrolling
//! - Scroll-to-index with offset calculation
//! - Height cache management

use anyhow::Result;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default estimated height for items not yet measured.
const DEFAULT_ESTIMATE: usize = 3;
/// Extra rows rendered above and below the viewport.
const OVERSCAN_ROWS: usize = 40;
/// Items rendered before the viewport has been measured.
const COLD_START_COUNT: usize = 30;
/// Maximum number of mounted items.
const MAX_MOUNTED_ITEMS: usize = 300;

// ---------------------------------------------------------------------------
// Scroll Item
// ---------------------------------------------------------------------------

/// An item in the virtual scroll list.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollItem {
    /// Unique identifier.
    pub id: String,
    /// Height in rows (0 = not yet measured).
    pub height: usize,
}

impl ScrollItem {
    /// Create a new scroll item with unknown height.
    pub fn new(id: impl Into<String>) -> Self {
        ScrollItem {
            id: id.into(),
            height: 0,
        }
    }

    /// Create a scroll item with a known height.
    pub fn with_height(id: impl Into<String>, height: usize) -> Self {
        ScrollItem {
            id: id.into(),
            height,
        }
    }

    /// Get the effective height (measured or estimated).
    pub fn effective_height(&self) -> usize {
        if self.height == 0 {
            DEFAULT_ESTIMATE
        } else {
            self.height
        }
    }
}

// ---------------------------------------------------------------------------
// Virtual Scroll State
// ---------------------------------------------------------------------------

/// State for the virtual scroll engine.
#[derive(Debug, Clone)]
pub struct VirtualScrollState {
    /// All items in the list.
    items: Vec<ScrollItem>,
    /// Viewport height in rows.
    viewport_height: usize,
    /// Current scroll offset in rows from the top.
    scroll_offset: usize,
    /// Whether auto-scroll to bottom is enabled.
    auto_scroll: bool,
}

impl VirtualScrollState {
    /// Create a new virtual scroll state with the given viewport height.
    pub fn new(viewport_height: usize) -> Self {
        VirtualScrollState {
            items: Vec::new(),
            viewport_height: viewport_height.max(1),
            scroll_offset: 0,
            auto_scroll: true,
        }
    }

    /// Set the items for the list.
    pub fn set_items(&mut self, items: Vec<ScrollItem>) {
        self.items = items;
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Get the total number of items.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Get a reference to the items.
    pub fn items(&self) -> &[ScrollItem] {
        &self.items
    }

    /// Update the viewport height.
    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height.max(1);
    }

    /// Get the viewport height.
    pub fn viewport_height(&self) -> usize {
        self.viewport_height
    }

    /// Get the current scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Set the scroll offset (clamped to valid range).
    pub fn set_scroll_offset(&mut self, offset: usize) {
        let max_offset = self.total_height().saturating_sub(self.viewport_height);
        self.scroll_offset = offset.min(max_offset);
    }

    /// Enable or disable auto-scroll.
    pub fn set_auto_scroll(&mut self, enabled: bool) {
        self.auto_scroll = enabled;
    }

    /// Check if auto-scroll is enabled.
    pub fn is_auto_scroll(&self) -> bool {
        self.auto_scroll
    }

    /// Update the measured height for an item.
    pub fn set_item_height(&mut self, index: usize, height: usize) {
        if index < self.items.len() {
            self.items[index].height = height.max(1);
        }
    }

    /// Compute cumulative offsets: offsets[i] = total height before item i.
    fn calculate_offsets(&self) -> Vec<usize> {
        let mut offsets = Vec::with_capacity(self.items.len() + 1);
        offsets.push(0);
        let mut cumulative: usize = 0;
        for item in &self.items {
            cumulative += item.effective_height();
            offsets.push(cumulative);
        }
        offsets
    }

    /// Compute the total height of all items.
    pub fn total_height(&self) -> usize {
        if self.items.is_empty() {
            return 0;
        }
        self.items.iter().map(|i| i.effective_height()).sum()
    }

    /// Compute the visible range `[start, end)`.
    pub fn visible_range(&self) -> (usize, usize) {
        if self.items.is_empty() {
            return (0, 0);
        }

        let offsets = self.calculate_offsets();
        let viewport_top = self.scroll_offset;
        let viewport_bottom = self.scroll_offset + self.viewport_height;

        // Find the first item whose bottom edge is past viewport_top.
        let mut start = 0;
        for i in 0..self.items.len() {
            let item_bottom = offsets[i + 1];
            if item_bottom > viewport_top {
                start = i;
                break;
            }
            start = i + 1;
        }

        if start >= self.items.len() {
            // We're scrolled past all items — show the last batch.
            start = self.items.len().saturating_sub(COLD_START_COUNT);
        }

        // Find the last item whose top edge is before viewport_bottom.
        let mut end = start;
        for (rel_i, &item_top) in offsets[start..self.items.len()].iter().enumerate() {
            if item_top >= viewport_bottom {
                end = start + rel_i;
                break;
            }
            end = start + rel_i + 1;
        }
        if end < start {
            end = start;
        }

        // Apply overscan.
        let overscan_start = start.saturating_sub(OVERSCAN_ROWS);
        let overscan_end = (end + OVERSCAN_ROWS).min(self.items.len());

        // Cap at max mounted items.
        let range_len = overscan_end - overscan_start;
        let capped_len = range_len.min(MAX_MOUNTED_ITEMS);

        (overscan_start, overscan_start + capped_len)
    }

    /// Get the spacer heights for the visible range.
    pub fn spacer_heights(&self) -> (usize, usize) {
        if self.items.is_empty() {
            return (0, 0);
        }

        let offsets = self.calculate_offsets();
        let (start, end) = self.visible_range();

        let top_spacer = if start < offsets.len() {
            offsets[start]
        } else {
            0
        };

        let total = self.total_height();
        let end_offset = if end < offsets.len() {
            offsets[end]
        } else {
            total
        };
        let bottom_spacer = total.saturating_sub(end_offset);

        (top_spacer, bottom_spacer)
    }

    /// Scroll to a specific item index.
    pub fn scroll_to_index(&mut self, index: usize) -> Result<()> {
        if index >= self.items.len() {
            return Err(anyhow::anyhow!(
                "index {} out of range (0..{})",
                index,
                self.items.len()
            ));
        }
        let offsets = self.calculate_offsets();
        let offset = offsets[index];
        self.set_scroll_offset(offset);
        self.auto_scroll = false;
        Ok(())
    }

    /// Scroll to the bottom of the list.
    pub fn scroll_to_bottom(&mut self) {
        let total = self.total_height();
        let max_offset = total.saturating_sub(self.viewport_height);
        self.scroll_offset = max_offset;
    }

    /// Scroll to the top of the list.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    /// Scroll by a number of rows (positive = down, negative = up).
    pub fn scroll_by(&mut self, rows: isize) {
        if rows > 0 {
            let new_offset = self.scroll_offset.saturating_add(rows as usize);
            self.set_scroll_offset(new_offset);
        } else {
            let new_offset = self.scroll_offset.saturating_sub((-rows) as usize);
            self.scroll_offset = new_offset;
        }
    }

    /// Check if the scroll is at the bottom.
    pub fn is_at_bottom(&self) -> bool {
        let total = self.total_height();
        self.scroll_offset + self.viewport_height >= total
    }

    /// Get the cold start range (for initial render before measurement).
    pub fn cold_start_range(&self) -> (usize, usize) {
        let count = COLD_START_COUNT.min(self.items.len());
        (0, count)
    }

    /// Get the offset for a specific item.
    pub fn item_offset(&self, index: usize) -> Option<usize> {
        if index >= self.items.len() {
            return None;
        }
        let offsets = self.calculate_offsets();
        Some(offsets[index])
    }

    /// Get all cumulative offsets.
    pub fn offsets(&self) -> Vec<usize> {
        self.calculate_offsets()
    }

    /// Find the item index at a given y-offset.
    pub fn index_at_offset(&self, y_offset: usize) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let offsets = self.calculate_offsets();
        // Binary search for the item containing y_offset.
        let mut lo: usize = 0;
        let mut hi: usize = self.items.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let item_start = offsets[mid];
            let item_end = offsets[mid + 1];
            if y_offset < item_start {
                hi = mid;
            } else if y_offset >= item_end {
                lo = mid + 1;
            } else {
                return Some(mid);
            }
        }
        if lo < self.items.len() {
            Some(lo)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_items(count: usize) -> Vec<ScrollItem> {
        (0..count)
            .map(|i| ScrollItem::with_height(format!("item-{i}"), 1))
            .collect()
    }

    fn make_items_with_height(count: usize, height: usize) -> Vec<ScrollItem> {
        (0..count)
            .map(|i| ScrollItem::with_height(format!("item-{i}"), height))
            .collect()
    }

    #[test]
    fn test_scroll_item_new() {
        let item = ScrollItem::new("test");
        assert_eq!(item.id, "test");
        assert_eq!(item.height, 0);
        assert_eq!(item.effective_height(), DEFAULT_ESTIMATE);
    }

    #[test]
    fn test_scroll_item_with_height() {
        let item = ScrollItem::with_height("test", 5);
        assert_eq!(item.height, 5);
        assert_eq!(item.effective_height(), 5);
    }

    #[test]
    fn test_virtual_scroll_new() {
        let vs = VirtualScrollState::new(24);
        assert_eq!(vs.viewport_height(), 24);
        assert_eq!(vs.item_count(), 0);
        assert_eq!(vs.total_height(), 0);
        assert!(vs.is_auto_scroll());
    }

    #[test]
    fn test_set_items() {
        let mut vs = VirtualScrollState::new(24);
        vs.set_items(make_items(10));
        assert_eq!(vs.item_count(), 10);
        assert_eq!(vs.total_height(), 10);
    }

    #[test]
    fn test_total_height() {
        let mut vs = VirtualScrollState::new(24);
        vs.set_items(make_items_with_height(5, 3));
        assert_eq!(vs.total_height(), 15);
    }

    #[test]
    fn test_visible_range_empty() {
        let vs = VirtualScrollState::new(24);
        assert_eq!(vs.visible_range(), (0, 0));
    }

    #[test]
    fn test_visible_range_basic() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(100));
        let (start, end) = vs.visible_range();
        assert!(end > 0);
        assert!(end <= 100);
        assert!(start < end);
    }

    #[test]
    fn test_scroll_to_top() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(100));
        vs.scroll_to_top();
        assert_eq!(vs.scroll_offset(), 0);
    }

    #[test]
    fn test_scroll_to_bottom() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(100));
        vs.scroll_to_bottom();
        assert!(vs.is_at_bottom());
    }

    #[test]
    fn test_scroll_to_index() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(100));
        vs.scroll_to_index(50).expect("scroll");
        assert!(vs.scroll_offset() > 0);
    }

    #[test]
    fn test_scroll_to_index_out_of_range() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(10));
        let result = vs.scroll_to_index(20);
        assert!(result.is_err());
    }

    #[test]
    fn test_scroll_by() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(100));
        vs.scroll_to_top();
        assert_eq!(vs.scroll_offset(), 0);
        vs.scroll_by(5);
        assert_eq!(vs.scroll_offset(), 5);
        vs.scroll_by(-3);
        assert_eq!(vs.scroll_offset(), 2);
    }

    #[test]
    fn test_scroll_by_clamp() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(5));
        vs.scroll_by(-10);
        assert_eq!(vs.scroll_offset(), 0);
    }

    #[test]
    fn test_set_item_height() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(5));
        assert_eq!(vs.total_height(), 5);
        vs.set_item_height(2, 10);
        assert_eq!(vs.total_height(), 14); // 1+1+10+1+1
    }

    #[test]
    fn test_spacer_heights() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(100));
        vs.scroll_to_top();
        let (top, bottom) = vs.spacer_heights();
        assert_eq!(top, 0);
        assert!(bottom > 0);
    }

    #[test]
    fn test_item_offset() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items_with_height(5, 3));
        assert_eq!(vs.item_offset(0), Some(0));
        assert_eq!(vs.item_offset(1), Some(3));
        assert_eq!(vs.item_offset(4), Some(12));
        assert_eq!(vs.item_offset(5), None);
    }

    #[test]
    fn test_index_at_offset() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items_with_height(5, 3));
        assert_eq!(vs.index_at_offset(0), Some(0));
        assert_eq!(vs.index_at_offset(2), Some(0));
        assert_eq!(vs.index_at_offset(3), Some(1));
        assert_eq!(vs.index_at_offset(14), Some(4));
    }

    #[test]
    fn test_index_at_offset_empty() {
        let vs = VirtualScrollState::new(10);
        assert_eq!(vs.index_at_offset(0), None);
    }

    #[test]
    fn test_cold_start_range() {
        let vs = VirtualScrollState::new(10);
        assert_eq!(vs.cold_start_range(), (0, 0));
    }

    #[test]
    fn test_cold_start_range_with_items() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(100));
        let (start, end) = vs.cold_start_range();
        assert_eq!(start, 0);
        assert_eq!(end, COLD_START_COUNT);
    }

    #[test]
    fn test_set_viewport_height() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_viewport_height(50);
        assert_eq!(vs.viewport_height(), 50);
    }

    #[test]
    fn test_set_viewport_height_zero_clamped() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_viewport_height(0);
        assert_eq!(vs.viewport_height(), 1);
    }

    #[test]
    fn test_offsets() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items_with_height(3, 5));
        let offsets = vs.offsets();
        assert_eq!(offsets, vec![0, 5, 10, 15]);
    }

    #[test]
    fn test_auto_scroll_disabled_on_manual_scroll() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(100));
        assert!(vs.is_auto_scroll());
        vs.scroll_to_index(10).expect("ok");
        assert!(!vs.is_auto_scroll());
    }

    #[test]
    fn test_visible_range_with_overscan() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(200));
        vs.scroll_to_top();
        let (start, end) = vs.visible_range();
        // With overscan, start should be 0 (clamped).
        assert_eq!(start, 0);
        // End should include viewport + overscan.
        assert!(end > 10);
    }

    #[test]
    fn test_scroll_by_past_end() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(20));
        vs.scroll_by(1000);
        // Should be clamped to max offset.
        let max_offset = 20 - 10; // total_height - viewport
        assert_eq!(vs.scroll_offset(), max_offset);
    }

    #[test]
    fn test_items_accessor() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_items(make_items(3));
        assert_eq!(vs.items().len(), 3);
        assert_eq!(vs.items()[0].id, "item-0");
    }

    #[test]
    fn test_set_auto_scroll() {
        let mut vs = VirtualScrollState::new(10);
        vs.set_auto_scroll(false);
        assert!(!vs.is_auto_scroll());
        vs.set_auto_scroll(true);
        assert!(vs.is_auto_scroll());
    }
}
