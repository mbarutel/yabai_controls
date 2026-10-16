//! Last-focus memory: the current and the previously focused id, so a
//! binding can jump back to what was focused before.
//!
//! Every focus change yabai reports shifts `current` into `previous`.
//! [`back`] swaps the two and returns the new current, which the caller must
//! actually focus; yabai's report of that focus then matches `current` and
//! is a no-op. Pressing "previous" twice therefore toggles between the last
//! two, alt-tab style. There is no forward.
//!
//! [`back`]: LastFocus::back

#[derive(Debug, Clone, Default)]
pub struct LastFocus<T> {
    current: Option<T>,
    previous: Option<T>,
}

impl<T: Copy + Eq> LastFocus<T> {
    pub fn new() -> Self {
        LastFocus {
            current: None,
            previous: None,
        }
    }

    pub fn current(&self) -> Option<T> {
        self.current
    }

    /// Focused before the current one; what [`back`](Self::back) would go to.
    pub fn previous(&self) -> Option<T> {
        self.previous
    }

    /// Records a focus change reported by yabai. Re-reports of the current
    /// focus (including the one our own `back` causes) change nothing.
    pub fn record(&mut self, id: T) {
        if self.current == Some(id) {
            return;
        }
        if let Some(current) = self.current {
            self.previous = Some(current);
        }
        self.current = Some(id);
    }

    /// Swaps current and previous and returns the new current. The caller
    /// must then actually focus it. Swapping eagerly, rather than on yabai's
    /// report, keeps a fast double press toggling correctly.
    pub fn back(&mut self) -> Option<T> {
        let target = self.previous?;
        self.previous = self.current;
        self.current = Some(target);
        Some(target)
    }

    /// Forgets ids that no longer exist (closed windows, destroyed spaces).
    /// Losing `current` keeps `previous`: after a window closes and yabai
    /// focuses something else, "previous" still points to where you were
    /// before that window.
    pub fn retain(&mut self, keep: impl Fn(&T) -> bool) {
        if self.current.is_some_and(|c| !keep(&c)) {
            self.current = None;
        }
        if self.previous.is_some_and(|p| !keep(&p)) {
            self.previous = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus(ids: &[u32]) -> LastFocus<u32> {
        let mut f = LastFocus::new();
        for id in ids {
            f.record(*id);
        }
        f
    }

    #[test]
    fn empty_has_nothing_and_back_is_none() {
        let mut f: LastFocus<u32> = LastFocus::new();
        assert_eq!(f.current(), None);
        assert_eq!(f.previous(), None);
        assert_eq!(f.back(), None);
    }

    #[test]
    fn records_keep_only_the_last_two() {
        let f = focus(&[1, 2, 3]);
        assert_eq!(f.current(), Some(3));
        assert_eq!(f.previous(), Some(2));
    }

    #[test]
    fn repeated_focus_of_current_is_not_recorded() {
        let f = focus(&[1, 2, 2, 2]);
        assert_eq!(f.current(), Some(2));
        assert_eq!(f.previous(), Some(1));
    }

    #[test]
    fn back_toggles_and_absorbs_its_own_report() {
        let mut f = focus(&[1, 2]);
        assert_eq!(f.back(), Some(1));
        // yabai reports the focus we caused: nothing changes.
        f.record(1);
        assert_eq!((f.current(), f.previous()), (Some(1), Some(2)));
        assert_eq!(f.back(), Some(2));
        f.record(2);
        assert_eq!((f.current(), f.previous()), (Some(2), Some(1)));
    }

    #[test]
    fn back_with_only_one_entry_is_none() {
        let mut f = focus(&[7]);
        assert_eq!(f.back(), None);
        assert_eq!(f.current(), Some(7));
    }

    #[test]
    fn retain_losing_current_keeps_previous_for_the_next_record() {
        let mut f = focus(&[1, 2]);
        f.retain(|id| *id != 2);
        assert_eq!((f.current(), f.previous()), (None, Some(1)));
        f.record(3);
        assert_eq!((f.current(), f.previous()), (Some(3), Some(1)));
    }

    #[test]
    fn retain_losing_previous_leaves_nothing_to_go_back_to() {
        let mut f = focus(&[1, 2]);
        f.retain(|id| *id != 1);
        assert_eq!((f.current(), f.previous()), (Some(2), None));
        assert_eq!(f.back(), None);
    }
}
