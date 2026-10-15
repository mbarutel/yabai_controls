//! Focus history: the trail of what was focused, with a cursor, so a
//! binding can step back to the previous window/space and forward again.
//!
//! Semantics are browser-like. Every externally caused focus change (a
//! click, `window --focus east`, an app opening a window) is appended after
//! the cursor and discards anything that was ahead of it. [`back`] and
//! [`forward`] only move the cursor; the focus change they cause is reported
//! by yabai like any other, so the caller tells the history to expect it and
//! that one report is absorbed instead of being appended.
//!
//! [`back`]: FocusHistory::back
//! [`forward`]: FocusHistory::forward

#[derive(Debug, Clone)]
pub struct FocusHistory<T> {
    /// Oldest first. Adjacent entries are always distinct.
    entries: Vec<T>,
    /// Index of the current focus in `entries`; meaningless when empty.
    cursor: usize,
    /// Focus change caused by `back`/`forward` that yabai has not yet
    /// reported.
    expected: Option<T>,
    capacity: usize,
}

impl<T: Copy + Eq> FocusHistory<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity >= 2, "history needs room for at least two entries");
        FocusHistory {
            entries: Vec::new(),
            cursor: 0,
            expected: None,
            capacity,
        }
    }

    pub fn current(&self) -> Option<T> {
        self.entries.get(self.cursor).copied()
    }

    /// Focused before the current one; what [`back`](Self::back) would go to.
    pub fn previous(&self) -> Option<T> {
        self.cursor
            .checked_sub(1)
            .and_then(|i| self.entries.get(i))
            .copied()
    }

    /// Where the trail continues after the current one; only present after
    /// a `back`. What [`forward`](Self::forward) would go to.
    pub fn next(&self) -> Option<T> {
        self.entries.get(self.cursor + 1).copied()
    }

    /// Records a focus change reported by yabai.
    pub fn record(&mut self, id: T) {
        if self.expected.take() == Some(id) {
            // Our own back/forward landed; the cursor already points at it.
            return;
        }
        if self.current() == Some(id) {
            return;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.cursor + 1);
        }
        self.entries.push(id);
        if self.entries.len() > self.capacity {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len() - 1;
    }

    /// Moves the cursor to the previous entry and returns it. The caller
    /// must then actually focus it; the resulting report is absorbed.
    pub fn back(&mut self) -> Option<T> {
        let target = self.previous()?;
        self.cursor -= 1;
        self.expected = Some(target);
        Some(target)
    }

    /// Counterpart of [`back`](Self::back).
    pub fn forward(&mut self) -> Option<T> {
        let target = self.next()?;
        self.cursor += 1;
        self.expected = Some(target);
        Some(target)
    }

    /// Drops entries that no longer exist (closed windows, destroyed
    /// spaces), keeping the cursor on the current entry or, if that is gone,
    /// on the nearest surviving earlier one (the oldest survivor if none).
    pub fn retain(&mut self, keep: impl Fn(&T) -> bool) {
        let mut kept = Vec::with_capacity(self.entries.len());
        let mut cursor = 0;
        for (i, entry) in self.entries.iter().enumerate() {
            if !keep(entry) || kept.last() == Some(entry) {
                continue;
            }
            if i <= self.cursor {
                cursor = kept.len();
            }
            kept.push(*entry);
        }
        self.entries = kept;
        self.cursor = cursor;
        if self.expected.is_some_and(|e| !keep(&e)) {
            self.expected = None;
        }
    }

    /// Oldest to newest, for diagnostics.
    pub fn entries(&self) -> &[T] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(ids: &[u32]) -> FocusHistory<u32> {
        let mut h = FocusHistory::new(8);
        for id in ids {
            h.record(*id);
        }
        h
    }

    #[test]
    fn empty_history_has_nothing() {
        let mut h: FocusHistory<u32> = FocusHistory::new(2);
        assert_eq!(h.current(), None);
        assert_eq!(h.previous(), None);
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), None);
    }

    #[test]
    fn records_build_a_trail_with_previous_but_no_next() {
        let h = history(&[1, 2, 3]);
        assert_eq!(h.current(), Some(3));
        assert_eq!(h.previous(), Some(2));
        assert_eq!(h.next(), None);
    }

    #[test]
    fn repeated_focus_of_current_is_not_recorded() {
        let h = history(&[1, 2, 2, 2]);
        assert_eq!(h.entries(), &[1, 2]);
    }

    #[test]
    fn back_and_forward_walk_the_trail_and_absorb_their_own_reports() {
        let mut h = history(&[1, 2, 3]);

        assert_eq!(h.back(), Some(2));
        h.record(2); // yabai reports the focus we caused
        assert_eq!(h.entries(), &[1, 2, 3], "absorbed, not appended");
        assert_eq!(
            (h.previous(), h.current(), h.next()),
            (Some(1), Some(2), Some(3))
        );

        assert_eq!(h.back(), Some(1));
        h.record(1);
        assert_eq!(h.back(), None, "nothing before the oldest");

        assert_eq!(h.forward(), Some(2));
        h.record(2);
        assert_eq!(h.forward(), Some(3));
        h.record(3);
        assert_eq!(h.forward(), None);
        assert_eq!(h.entries(), &[1, 2, 3]);
    }

    #[test]
    fn external_focus_after_back_discards_the_forward_trail() {
        let mut h = history(&[1, 2, 3]);
        h.back();
        h.record(2);

        h.record(9); // user clicked something else

        assert_eq!(h.entries(), &[1, 2, 9]);
        assert_eq!(h.next(), None);
        assert_eq!(h.previous(), Some(2));
    }

    #[test]
    fn unexpected_report_after_back_is_recorded_normally() {
        // back() targeted 2, but yabai focused 7 instead (2 vanished, say).
        let mut h = history(&[1, 2, 3]);
        assert_eq!(h.back(), Some(2));

        h.record(7);

        assert_eq!(h.entries(), &[1, 2, 7]);
        assert_eq!(h.current(), Some(7));
        h.record(2); // a later genuine focus of 2 must not be absorbed
        assert_eq!(h.entries(), &[1, 2, 7, 2]);
    }

    #[test]
    fn capacity_drops_the_oldest() {
        let mut h = FocusHistory::new(3);
        for id in [1, 2, 3, 4] {
            h.record(id);
        }
        assert_eq!(h.entries(), &[2, 3, 4]);
        assert_eq!(h.current(), Some(4));
    }

    #[test]
    fn retain_removes_dead_entries_and_keeps_cursor_on_current() {
        let mut h = history(&[1, 2, 3, 4]);
        h.back(); // cursor on 3
        h.record(3);

        h.retain(|id| *id != 2);

        assert_eq!(h.entries(), &[1, 3, 4]);
        assert_eq!(h.current(), Some(3));
        assert_eq!(h.previous(), Some(1));
        assert_eq!(h.next(), Some(4));
    }

    #[test]
    fn retain_moves_cursor_to_nearest_earlier_survivor_when_current_dies() {
        let mut h = history(&[1, 2, 3, 4]);
        h.back();
        h.record(3);

        h.retain(|id| *id != 3);

        assert_eq!(h.entries(), &[1, 2, 4]);
        assert_eq!(h.current(), Some(2));
        assert_eq!(h.next(), Some(4));
    }

    #[test]
    fn retain_merges_adjacent_duplicates_left_by_removal() {
        let mut h = history(&[1, 2, 1]);
        h.retain(|id| *id != 2);
        assert_eq!(h.entries(), &[1]);
        assert_eq!(h.current(), Some(1));
    }

    #[test]
    fn retain_of_everything_before_cursor_leaves_current_first() {
        let mut h = history(&[1, 2, 3]);
        h.retain(|id| *id == 3);
        assert_eq!(h.entries(), &[3]);
        assert_eq!(h.current(), Some(3));
        assert_eq!(h.previous(), None);
    }
}
