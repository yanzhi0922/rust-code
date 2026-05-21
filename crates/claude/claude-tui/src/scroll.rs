//! Virtual scroll engine for large message lists.
//!
//! Only renders messages within the visible viewport, supporting
//! 10 000+ messages without performance degradation.

/// Virtual scroll state — tracks which items are visible.
#[derive(Debug, Clone)]
pub struct VirtualScroll {
    /// Number of items (messages).
    total_items: usize,
    /// Cached height (in terminal rows) for each item.
    item_heights: Vec<usize>,
    /// Viewport height in terminal rows.
    viewport_height: usize,
    /// Scroll offset — number of rows skipped from the top.
    scroll_offset: usize,
    /// Whether to auto-scroll to the bottom when new items arrive.
    auto_scroll: bool,
}

impl VirtualScroll {
    /// Create a new scroll engine with the given viewport height (rows).
    pub fn new(viewport_height: usize) -> Self {
        VirtualScroll {
            total_items: 0,
            item_heights: Vec::new(),
            viewport_height: viewport_height.max(1),
            scroll_offset: 0,
            auto_scroll: true,
        }
    }

    /// Set the total number of items. Preserves scroll position if possible.
    pub fn set_items(&mut self, count: usize) {
        self.total_items = count;
        if self.item_heights.len() < count {
            self.item_heights.resize(count, 1);
        } else {
            self.item_heights.truncate(count);
        }
        self.clamp_offset();
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Set the rendered height (rows) for a specific item.
    pub fn set_item_height(&mut self, index: usize, height: usize) {
        if index < self.item_heights.len() {
            self.item_heights[index] = height.max(1);
        }
    }

    /// Update the viewport height and re-clamp the scroll offset.
    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height.max(1);
        self.clamp_offset();
    }

    /// Return the visible item index range `[start, end)`.
    ///
    /// Uses the cached item heights to compute which items fall within
    /// the viewport starting at `scroll_offset`.
    pub fn visible_range(&self) -> (usize, usize) {
        if self.total_items == 0 {
            return (0, 0);
        }

        let mut accumulated: usize = 0;
        let mut start = self.total_items; // sentinel: nothing visible
        let mut end = self.total_items;

        for (i, &h) in self.item_heights.iter().enumerate() {
            let item_top = accumulated;
            let item_bottom = accumulated + h;

            // Item is visible if its bottom edge is past the scroll offset
            // and its top edge is before the viewport end.
            let viewport_end = self.scroll_offset + self.viewport_height;

            if item_bottom > self.scroll_offset && item_top < viewport_end {
                if start == self.total_items {
                    start = i;
                }
                end = i + 1;
            }

            accumulated += h;
            if item_top >= viewport_end {
                break;
            }
        }

        (start, end.min(self.total_items))
    }

    /// Scroll to an absolute row offset.
    pub fn scroll_to(&mut self, offset: usize) {
        self.scroll_offset = offset.min(self.max_offset());
        self.auto_scroll = self.scroll_offset >= self.max_offset();
    }

    /// Scroll to the very top.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = false;
    }

    /// Scroll to the very bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.max_offset();
        self.auto_scroll = true;
    }

    /// Scroll up by `n` rows.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.auto_scroll = false;
    }

    /// Scroll down by `n` rows.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = (self.scroll_offset + n).min(self.max_offset());
        self.auto_scroll = self.scroll_offset >= self.max_offset();
    }

    /// Scroll up by half a viewport.
    pub fn page_up(&mut self) {
        self.scroll_up(self.viewport_height / 2);
    }

    /// Scroll down by half a viewport.
    pub fn page_down(&mut self) {
        self.scroll_down(self.viewport_height / 2);
    }

    /// Whether the viewport is at the bottom.
    pub fn is_at_bottom(&self) -> bool {
        self.scroll_offset >= self.max_offset()
    }

    /// Total height of all items in rows.
    pub fn total_height(&self) -> usize {
        self.item_heights.iter().sum()
    }

    /// Maximum valid scroll offset.
    pub fn max_offset(&self) -> usize {
        let total = self.total_height();
        total.saturating_sub(self.viewport_height)
    }

    /// Current scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Whether auto-scroll is enabled.
    pub fn auto_scroll(&self) -> bool {
        self.auto_scroll
    }

    /// Clamp scroll_offset to a valid range.
    fn clamp_offset(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.max_offset());
    }
}

impl Default for VirtualScroll {
    fn default() -> Self {
        Self::new(24)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_scroll_has_zero_items() {
        let s = VirtualScroll::new(10);
        assert_eq!(s.total_items, 0);
        assert_eq!(s.visible_range(), (0, 0));
    }

    #[test]
    fn set_items_updates_count() {
        let mut s = VirtualScroll::new(10);
        s.set_items(5);
        assert_eq!(s.total_items, 5);
    }

    #[test]
    fn visible_range_with_uniform_heights() {
        let mut s = VirtualScroll::new(10);
        s.set_items(20);
        s.auto_scroll = false;
        s.scroll_to(0);
        // Each item has height 1, offset 0, viewport 10 → items 0..10
        let (start, end) = s.visible_range();
        assert_eq!(start, 0);
        assert!(end > 0);
        assert!(end <= 20);
    }

    #[test]
    fn scroll_down_shifts_visible_range() {
        let mut s = VirtualScroll::new(10);
        s.set_items(20);
        s.auto_scroll = false;
        s.scroll_to(0);
        s.scroll_down(5);
        let (start, end) = s.visible_range();
        // After scrolling down 5 rows, start should be >= 5
        assert!(start >= 5);
        assert!(end > start);
    }

    #[test]
    fn scroll_to_bottom() {
        let mut s = VirtualScroll::new(10);
        s.set_items(20);
        s.scroll_to_bottom();
        assert!(s.is_at_bottom());
        let (start, end) = s.visible_range();
        assert_eq!(start, 10);
        assert_eq!(end, 20);
    }

    #[test]
    fn scroll_to_top() {
        let mut s = VirtualScroll::new(10);
        s.set_items(20);
        s.scroll_to_bottom();
        s.scroll_to_top();
        assert_eq!(s.scroll_offset(), 0);
        assert!(!s.is_at_bottom());
    }

    #[test]
    fn page_up_and_page_down() {
        let mut s = VirtualScroll::new(10);
        s.set_items(100);
        s.auto_scroll = false;
        s.scroll_down(50);
        let offset_before = s.scroll_offset();
        s.page_up();
        assert!(s.scroll_offset() < offset_before);
        s.page_down();
        // After page_up + page_down, should be close to original.
        assert!(s.scroll_offset() >= offset_before - 1);
    }

    #[test]
    fn variable_item_heights() {
        let mut s = VirtualScroll::new(10);
        s.set_items(5);
        s.set_item_height(0, 3);
        s.set_item_height(1, 3);
        s.set_item_height(2, 3);
        s.set_item_height(3, 3);
        s.set_item_height(4, 3);
        // Total height = 15, viewport = 10, offset = 0
        // Items 0..3 visible (heights 3+3+3+3 = 12 > 10)
        s.auto_scroll = false;
        s.scroll_to(0);
        let (start, end) = s.visible_range();
        assert_eq!(start, 0);
        // Items 0,1,2,3 should all be at least partially visible
        assert!(end >= 3);
    }

    #[test]
    fn auto_scroll_on_new_items() {
        let mut s = VirtualScroll::new(10);
        s.set_items(5);
        assert!(s.auto_scroll);
        // Adding more items should keep auto-scroll
        s.set_items(10);
        assert!(s.auto_scroll);
        assert!(s.is_at_bottom());
    }

    #[test]
    fn max_offset_with_small_content() {
        let mut s = VirtualScroll::new(20);
        s.set_items(3); // total height = 3
        assert_eq!(s.max_offset(), 0);
    }

    #[test]
    fn set_viewport_height_reclamps() {
        let mut s = VirtualScroll::new(10);
        s.set_items(50);
        s.scroll_to_bottom();
        s.set_viewport_height(5);
        // Max offset should now be larger, and offset should be clamped
        assert!(s.scroll_offset() <= s.max_offset());
    }
}
