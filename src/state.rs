//! In-memory snapshot of yabai's displays, spaces and windows.
//!
//! Owned by the dispatch loop. Refreshed in full on startup and on
//! space/display events; patched per-window on window events. Also keeps
//! the current and previous focused window and space (see [`LastFocus`]) so
//! bindings can jump back to the previously focused one.

use std::collections::HashMap;

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
        self.spaces = client.spaces()?;
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
        let spaces = &self.spaces;
        self.space_focus
            .retain(|id| spaces.iter().any(|s| s.id == *id));
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
                }
                // Space window lists changed too; cheap enough to refetch.
                self.spaces = client.spaces()?;
            }
            WindowCreated | WindowFocused | WindowMoved | WindowResized | WindowMinimized
            | WindowDeminimized | WindowTitleChanged => {
                if let Some(id) = parse_first(args) {
                    self.refresh_window(client, id)?;
                    if event == WindowFocused && self.windows.contains_key(&id) {
                        self.window_focus.record(id);
                    }
                }
                if matches!(
                    event,
                    WindowCreated | WindowFocused | WindowMinimized | WindowDeminimized
                ) {
                    self.spaces = client.spaces()?;
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

    pub fn focused_display(&self) -> Option<&Display> {
        self.displays.iter().find(|d| d.has_focus)
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
}

fn parse_first(args: &[String]) -> Option<WindowId> {
    args.first().and_then(|a| a.parse().ok())
}
