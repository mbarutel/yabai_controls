//! In-memory snapshot of yabai's displays, spaces and windows.
//!
//! Owned by the dispatch loop. Refreshed in full on startup and on
//! space/display events; patched per-window on window events. Also keeps
//! the current and previous focused window and space (see [`LastFocus`]) so
//! bindings can jump back to the previously focused one.

use std::collections::HashMap;
use std::io::{self, Write};

use anyhow::Result;
use tracing::{debug, warn};

use crate::focus::LastFocus;
use crate::yabai::Client;
use crate::yabai::model::{Display, Space, SpaceId, Window, WindowId};
use crate::yabai::signal::SignalEvent;

#[derive(Debug, Clone, Default)]
pub struct State {
    pub displays: Vec<Display>,
    pub spaces: Vec<Space>,
    pub windows: HashMap<WindowId, Window>,
    /// Current and previous focused window, by id (`window --focus <id>`
    /// works).
    pub window_focus: LastFocus<WindowId>,
    /// Current and previous focused space, by id: indices shift when spaces
    /// are created or destroyed, ids do not. Resolve to an index to focus.
    pub space_focus: LastFocus<SpaceId>,
    /// When each window was last focused, as a tick of `focus_clock`
    /// (higher is more recent). Fed by `window_focused` signals; lets an
    /// app binding pick the window you used last.
    pub last_focused: HashMap<WindowId, u64>,
    focus_clock: u64,
}

impl State {
    pub fn fetch(client: &Client) -> Result<State> {
        let mut state = State::default();
        state.refresh_all(client)?;
        Ok(state)
    }

    /// Re-queries everything. The focus memories are pruned to what still
    /// exists; one without a current entry (startup, or everything closed)
    /// is seeded with the current focus. Otherwise they are fed only by
    /// signals: a snapshot taken between our own `back` and yabai's report
    /// of it would still show the old focus and undo the swap.
    pub fn refresh_all(&mut self, client: &Client) -> Result<()> {
        self.displays = client.displays()?;
        self.refresh_spaces(client)?;
        self.windows = client.windows()?.into_iter().map(|w| (w.id, w)).collect();
        self.prune_focus();
        if self.space_focus.current().is_none()
            && let Some(space) = self.focused_space()
        {
            self.space_focus.record(space.id);
        }
        if self.window_focus.current().is_none()
            && let Some(window) = self.focused_window()
        {
            self.window_focus.record(window.id);
        }
        if let Some(id) = self.focused_window().map(|w| w.id) {
            self.touch(id);
        }
        debug!(
            displays = self.displays.len(),
            spaces = self.spaces.len(),
            windows = self.windows.len(),
            "state refreshed"
        );
        Ok(())
    }

    /// Drops closed windows and destroyed spaces from the focus memories.
    pub fn prune_focus(&mut self) {
        let windows = &self.windows;
        self.window_focus.retain(|id| windows.contains_key(id));
        self.last_focused.retain(|id, _| windows.contains_key(id));
        let spaces = &self.spaces;
        self.space_focus
            .retain(|id| spaces.iter().any(|s| s.id == *id));
    }

    /// Marks `id` as the most recently focused window.
    fn touch(&mut self, id: WindowId) {
        self.focus_clock += 1;
        self.last_focused.insert(id, self.focus_clock);
    }

    /// The window of `app` an app binding should focus: its next window by
    /// id when one of them is focused already (so repeats cycle through
    /// all of them, wrapping), otherwise the one used most recently.
    /// `None` when the app has no eligible window.
    pub fn pick_app_window(&self, app: &str) -> Option<WindowId> {
        let mut candidates: Vec<&Window> = self
            .windows
            .values()
            .filter(|w| w.app == app && crate::ring::eligible(w))
            .collect();
        candidates.sort_by_key(|w| w.id);
        match candidates.iter().position(|w| w.has_focus) {
            Some(i) => Some(candidates[(i + 1) % candidates.len()].id),
            None => candidates
                .iter()
                .max_by_key(|w| (self.last_focused.get(&w.id).copied().unwrap_or(0), w.id))
                .map(|w| w.id),
        }
    }

    /// Updates the snapshot in response to a yabai signal. `args` are the
    /// positional values forwarded by `notify` (see `SignalEvent::env_vars`).
    pub fn apply(&mut self, client: &Client, event: SignalEvent, args: &[String]) -> Result<()> {
        use SignalEvent::*;
        match event {
            WindowDestroyed => {
                if let Some(id) = parse_first(args) {
                    self.windows.remove(&id);
                    self.window_focus.retain(|w| *w != id);
                    self.last_focused.remove(&id);
                }
                // Space window lists changed too; cheap enough to refetch.
                self.refresh_spaces(client)?;
            }
            WindowCreated | WindowFocused | WindowMoved | WindowResized | WindowMinimized
            | WindowDeminimized | WindowTitleChanged => {
                if let Some(id) = parse_first(args) {
                    self.refresh_window(client, id)?;
                    if event == WindowFocused && self.windows.contains_key(&id) {
                        self.window_focus.record(id);
                        self.touch(id);
                    }
                }
                if matches!(
                    event,
                    WindowCreated | WindowFocused | WindowMinimized | WindowDeminimized
                ) {
                    self.refresh_spaces(client)?;
                }
            }
            SpaceChanged => {
                // Record from the signal, not the refreshed snapshot, so a
                // focus we caused ourselves is matched against the exact id.
                if let Some(id) = args.first().and_then(|a| a.parse::<SpaceId>().ok()) {
                    self.space_focus.record(id);
                }
                self.refresh_all(client)?;
            }
            SpaceCreated | SpaceDestroyed | DisplayAdded | DisplayRemoved | DisplayMoved
            | DisplayResized | DisplayChanged | SystemWoke | DockDidRestart => {
                self.refresh_all(client)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Re-queries spaces only; cheap, and window signals change their
    /// window lists and `has_focus`.
    fn refresh_spaces(&mut self, client: &Client) -> Result<()> {
        self.spaces = client.spaces()?;
        Ok(())
    }

    fn refresh_window(&mut self, client: &Client, id: WindowId) -> Result<()> {
        match client.window(id) {
            Ok(window) => {
                if window.has_focus {
                    for w in self.windows.values_mut() {
                        w.has_focus = false;
                    }
                }
                self.windows.insert(id, window);
            }
            Err(err) => {
                // The window may already be gone by the time we query.
                warn!(id, %err, "could not refresh window; dropping it");
                self.windows.remove(&id);
            }
        }
        Ok(())
    }

    pub fn focused_space(&self) -> Option<&Space> {
        self.spaces.iter().find(|s| s.has_focus)
    }

    pub fn focused_window(&self) -> Option<&Window> {
        self.windows.values().find(|w| w.has_focus)
    }

    pub fn space_by_index(&self, index: u32) -> Option<&Space> {
        self.spaces.iter().find(|s| s.index == index)
    }

    pub fn space_by_id(&self, id: SpaceId) -> Option<&Space> {
        self.spaces.iter().find(|s| s.id == id)
    }

    /// The space a known window is on.
    pub fn space_of_window(&self, id: WindowId) -> Option<&Space> {
        self.windows
            .get(&id)
            .and_then(|w| self.space_by_index(w.space))
    }

    /// Standard windows on a space, in stable id order.
    pub fn windows_on_space(&self, space_index: u32) -> Vec<&Window> {
        let mut windows: Vec<&Window> = self
            .windows
            .values()
            .filter(|w| w.space == space_index && w.is_standard())
            .collect();
        windows.sort_by_key(|w| w.id);
        windows
    }

    /// Human-readable dump for the `state` subcommand: displays, their
    /// spaces, and each space's windows; `*` marks focus.
    pub fn print(&self, out: &mut impl Write) -> io::Result<()> {
        fn star(focused: bool) -> &'static str {
            if focused { " *" } else { "" }
        }
        for display in &self.displays {
            writeln!(
                out,
                "display {} ({}x{}){}",
                display.index,
                display.frame.w,
                display.frame.h,
                star(display.has_focus)
            )?;
            for space in display
                .spaces
                .iter()
                .filter_map(|i| self.space_by_index(*i))
            {
                let label = if space.label.is_empty() {
                    String::new()
                } else {
                    format!(" \"{}\"", space.label)
                };
                writeln!(
                    out,
                    "  space {} [{:?}]{}{label}",
                    space.index,
                    space.layout,
                    star(space.has_focus)
                )?;
                for window in self.windows_on_space(space.index) {
                    writeln!(
                        out,
                        "    {:>6} {:<20} {}{}",
                        window.id,
                        window.app,
                        window.title,
                        star(window.has_focus)
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn parse_first(args: &[String]) -> Option<WindowId> {
    args.first().and_then(|a| a.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: WindowId, app: &str, has_focus: bool) -> Window {
        Window {
            id,
            app: app.into(),
            role: "AXWindow".into(),
            subrole: "AXStandardWindow".into(),
            has_focus,
            ..Window::default()
        }
    }

    fn state(windows: Vec<Window>) -> State {
        State {
            windows: windows.into_iter().map(|w| (w.id, w)).collect(),
            ..State::default()
        }
    }

    #[test]
    fn picks_most_recently_focused_window_of_app() {
        let mut s = state(vec![
            win(1, "Chrome", false),
            win(2, "Chrome", false),
            win(3, "Ghostty", true),
        ]);
        s.touch(2);
        s.touch(1);
        s.touch(3);
        assert_eq!(s.pick_app_window("Chrome"), Some(1));
    }

    #[test]
    fn cycles_by_id_with_wrap_when_app_is_already_focused() {
        let s = state(vec![
            win(5, "Chrome", false),
            win(9, "Chrome", true),
            win(2, "Chrome", false),
        ]);
        // sorted ids 2, 5, 9; focused is 9 so wrap to 2 — not the most
        // recent, which would just toggle between two windows.
        assert_eq!(s.pick_app_window("Chrome"), Some(2));
    }

    #[test]
    fn single_focused_window_picks_itself() {
        let s = state(vec![win(1, "Chrome", true)]);
        assert_eq!(s.pick_app_window("Chrome"), Some(1));
    }

    #[test]
    fn skips_minimized_and_non_standard_windows() {
        let mut minimized = win(1, "Chrome", false);
        minimized.is_minimized = true;
        let mut dialog = win(2, "Chrome", false);
        dialog.subrole = "AXDialog".into();
        let s = state(vec![minimized, dialog, win(3, "Other", false)]);
        assert_eq!(s.pick_app_window("Chrome"), None);
        assert_eq!(s.pick_app_window("Nope"), None);
    }

    #[test]
    fn untouched_windows_fall_back_to_highest_id() {
        let s = state(vec![win(4, "Chrome", false), win(7, "Chrome", false)]);
        assert_eq!(s.pick_app_window("Chrome"), Some(7));
    }

    #[test]
    fn prune_drops_recency_of_closed_windows() {
        let mut s = state(vec![win(1, "Chrome", false)]);
        s.touch(1);
        s.touch(99);
        s.prune_focus();
        assert_eq!(s.last_focused.keys().copied().collect::<Vec<_>>(), vec![1]);
    }
}
