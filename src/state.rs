//! In-memory snapshot of yabai's displays, spaces and windows.
//!
//! Owned by the dispatch loop. Refreshed in full on startup and on
//! space/display events; patched per-window on window events.

use std::collections::HashMap;

use anyhow::Result;
use tracing::{debug, warn};

use crate::yabai::Client;
use crate::yabai::model::{Display, Space, Window, WindowId};
use crate::yabai::signal::SignalEvent;

#[derive(Debug, Default, Clone)]
pub struct State {
    pub displays: Vec<Display>,
    pub spaces: Vec<Space>,
    pub windows: HashMap<WindowId, Window>,
}

impl State {
    pub fn fetch(client: &Client) -> Result<State> {
        let mut state = State::default();
        state.refresh_all(client)?;
        Ok(state)
    }

    pub fn refresh_all(&mut self, client: &Client) -> Result<()> {
        self.displays = client.displays()?;
        self.spaces = client.spaces()?;
        self.windows = client.windows()?.into_iter().map(|w| (w.id, w)).collect();
        debug!(
            displays = self.displays.len(),
            spaces = self.spaces.len(),
            windows = self.windows.len(),
            "state refreshed"
        );
        Ok(())
    }

    /// Updates the snapshot in response to a yabai signal. `args` are the
    /// positional values forwarded by `notify` (see `SignalEvent::env_vars`).
    pub fn apply(&mut self, client: &Client, event: SignalEvent, args: &[String]) -> Result<()> {
        use SignalEvent::*;
        match event {
            WindowDestroyed => {
                if let Some(id) = parse_first(args) {
                    self.windows.remove(&id);
                }
                // Space window lists changed too; cheap enough to refetch.
                self.spaces = client.spaces()?;
            }
            WindowCreated | WindowFocused | WindowMoved | WindowResized | WindowMinimized
            | WindowDeminimized | WindowTitleChanged => {
                if let Some(id) = parse_first(args) {
                    self.refresh_window(client, id)?;
                }
                if matches!(
                    event,
                    WindowCreated | WindowFocused | WindowMinimized | WindowDeminimized
                ) {
                    self.spaces = client.spaces()?;
                }
            }
            SpaceCreated | SpaceDestroyed | SpaceChanged | DisplayAdded | DisplayRemoved
            | DisplayMoved | DisplayResized | DisplayChanged | SystemWoke | DockDidRestart => {
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
