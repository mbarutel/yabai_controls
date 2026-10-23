//! Registration of yabai signals that forward events back to this daemon.
//!
//! yabai signals run a shell command. We register one per event that invokes
//! `yabai_controls notify <event> <args...>`, which connects to our own socket
//! (see `crate::events`). Signals are labelled `yabai_controls_<event>` so
//! re-registering on restart replaces rather than duplicates them.

use anyhow::Result;
use tracing::{debug, warn};

use super::Client;
use crate::actions::shell_quote;

pub const LABEL_PREFIX: &str = "yabai_controls_";

/// Events yabai can emit, with the environment variables it passes along.
/// Mirrors the SIGNAL section of `man yabai`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalEvent {
    ApplicationLaunched,
    ApplicationTerminated,
    ApplicationFrontSwitched,
    ApplicationActivated,
    ApplicationDeactivated,
    ApplicationVisible,
    ApplicationHidden,
    WindowCreated,
    WindowDestroyed,
    WindowFocused,
    WindowMoved,
    WindowResized,
    WindowMinimized,
    WindowDeminimized,
    WindowTitleChanged,
    SpaceCreated,
    SpaceDestroyed,
    SpaceChanged,
    DisplayAdded,
    DisplayRemoved,
    DisplayMoved,
    DisplayResized,
    DisplayChanged,
    MissionControlEnter,
    MissionControlExit,
    DockDidChangePref,
    DockDidRestart,
    MenuBarHiddenChanged,
    SystemWoke,
}

impl SignalEvent {
    pub const ALL: &[SignalEvent] = &[
        Self::ApplicationLaunched,
        Self::ApplicationTerminated,
        Self::ApplicationFrontSwitched,
        Self::ApplicationActivated,
        Self::ApplicationDeactivated,
        Self::ApplicationVisible,
        Self::ApplicationHidden,
        Self::WindowCreated,
        Self::WindowDestroyed,
        Self::WindowFocused,
        Self::WindowMoved,
        Self::WindowResized,
        Self::WindowMinimized,
        Self::WindowDeminimized,
        Self::WindowTitleChanged,
        Self::SpaceCreated,
        Self::SpaceDestroyed,
        Self::SpaceChanged,
        Self::DisplayAdded,
        Self::DisplayRemoved,
        Self::DisplayMoved,
        Self::DisplayResized,
        Self::DisplayChanged,
        Self::MissionControlEnter,
        Self::MissionControlExit,
        Self::DockDidChangePref,
        Self::DockDidRestart,
        Self::MenuBarHiddenChanged,
        Self::SystemWoke,
    ];

    /// The default set the daemon subscribes to: enough to keep `State`
    /// current without a shell spawn for every app activation.
    pub const DEFAULT: &[SignalEvent] = &[
        Self::WindowCreated,
        Self::WindowDestroyed,
        Self::WindowFocused,
        Self::WindowMoved,
        Self::WindowResized,
        Self::WindowMinimized,
        Self::WindowDeminimized,
        Self::WindowTitleChanged,
        Self::SpaceCreated,
        Self::SpaceDestroyed,
        Self::SpaceChanged,
        Self::DisplayAdded,
        Self::DisplayRemoved,
        Self::DisplayMoved,
        Self::DisplayResized,
        Self::DisplayChanged,
        Self::SystemWoke,
    ];

    /// The `event=` name yabai expects.
    pub fn name(self) -> &'static str {
        match self {
            Self::ApplicationLaunched => "application_launched",
            Self::ApplicationTerminated => "application_terminated",
            Self::ApplicationFrontSwitched => "application_front_switched",
            Self::ApplicationActivated => "application_activated",
            Self::ApplicationDeactivated => "application_deactivated",
            Self::ApplicationVisible => "application_visible",
            Self::ApplicationHidden => "application_hidden",
            Self::WindowCreated => "window_created",
            Self::WindowDestroyed => "window_destroyed",
            Self::WindowFocused => "window_focused",
            Self::WindowMoved => "window_moved",
            Self::WindowResized => "window_resized",
            Self::WindowMinimized => "window_minimized",
            Self::WindowDeminimized => "window_deminimized",
            Self::WindowTitleChanged => "window_title_changed",
            Self::SpaceCreated => "space_created",
            Self::SpaceDestroyed => "space_destroyed",
            Self::SpaceChanged => "space_changed",
            Self::DisplayAdded => "display_added",
            Self::DisplayRemoved => "display_removed",
            Self::DisplayMoved => "display_moved",
            Self::DisplayResized => "display_resized",
            Self::DisplayChanged => "display_changed",
            Self::MissionControlEnter => "mission_control_enter",
            Self::MissionControlExit => "mission_control_exit",
            Self::DockDidChangePref => "dock_did_change_pref",
            Self::DockDidRestart => "dock_did_restart",
            Self::MenuBarHiddenChanged => "menu_bar_hidden_changed",
            Self::SystemWoke => "system_woke",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|e| e.name() == name)
    }

    /// Environment variables yabai sets for this event, in the order we
    /// forward them as positional arguments to `notify`.
    pub fn env_vars(self) -> &'static [&'static str] {
        use SignalEvent::*;
        match self {
            ApplicationLaunched
            | ApplicationTerminated
            | ApplicationActivated
            | ApplicationDeactivated
            | ApplicationVisible
            | ApplicationHidden => &["YABAI_PROCESS_ID"],
            ApplicationFrontSwitched => &["YABAI_PROCESS_ID", "YABAI_RECENT_PROCESS_ID"],
            WindowCreated | WindowDestroyed | WindowFocused | WindowMoved | WindowResized
            | WindowMinimized | WindowDeminimized | WindowTitleChanged => &["YABAI_WINDOW_ID"],
            SpaceCreated => &["YABAI_SPACE_ID", "YABAI_SPACE_INDEX"],
            SpaceDestroyed => &["YABAI_SPACE_ID"],
            SpaceChanged => &[
                "YABAI_SPACE_ID",
                "YABAI_SPACE_INDEX",
                "YABAI_RECENT_SPACE_ID",
                "YABAI_RECENT_SPACE_INDEX",
            ],
            DisplayAdded | DisplayMoved | DisplayResized => {
                &["YABAI_DISPLAY_ID", "YABAI_DISPLAY_INDEX"]
            }
            DisplayRemoved => &["YABAI_DISPLAY_ID"],
            DisplayChanged => &[
                "YABAI_DISPLAY_ID",
                "YABAI_DISPLAY_INDEX",
                "YABAI_RECENT_DISPLAY_ID",
                "YABAI_RECENT_DISPLAY_INDEX",
            ],
            MissionControlEnter | MissionControlExit => &["YABAI_MISSION_CONTROL_MODE"],
            DockDidChangePref | DockDidRestart | MenuBarHiddenChanged | SystemWoke => &[],
        }
    }

    pub fn label(self) -> String {
        format!("{LABEL_PREFIX}{}", self.name())
    }

    /// The shell command yabai will run. `notify_bin` is the absolute path of
    /// this executable.
    pub fn action(self, notify_bin: &str) -> String {
        let mut cmd = format!("{} notify {}", shell_quote(notify_bin), self.name());
        for var in self.env_vars() {
            cmd.push_str(&format!(" \"${var}\""));
        }
        cmd
    }
}

impl Client {
    /// Registers (or replaces) a signal for each event.
    pub fn register_signals(&self, events: &[SignalEvent], notify_bin: &str) -> Result<()> {
        for &event in events {
            let label = event.label();
            let action = event.action(notify_bin);
            debug!(%label, %action, "registering yabai signal");
            // --add with an existing label replaces that signal in yabai >= 4.
            self.send(&[
                "signal",
                "--add",
                &format!("label={label}"),
                &format!("event={}", event.name()),
                &format!("action={action}"),
            ])?;
        }
        Ok(())
    }

    /// Removes every signal registered by `register_signals`.
    pub fn remove_signals(&self, events: &[SignalEvent]) {
        for &event in events {
            if let Err(err) = self.send(&["signal", "--remove", &event.label()]) {
                warn!(label = %event.label(), %err, "failed to remove yabai signal");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_forwards_env_vars_as_positional_args() {
        let action = SignalEvent::SpaceChanged.action("/usr/local/bin/yabai_controls");
        assert_eq!(
            action,
            "/usr/local/bin/yabai_controls notify space_changed \"$YABAI_SPACE_ID\" \"$YABAI_SPACE_INDEX\" \"$YABAI_RECENT_SPACE_ID\" \"$YABAI_RECENT_SPACE_INDEX\""
        );
    }

    #[test]
    fn action_quotes_paths_with_spaces() {
        let action = SignalEvent::SystemWoke.action("/Users/me/my tools/yabai_controls");
        assert_eq!(
            action,
            "'/Users/me/my tools/yabai_controls' notify system_woke"
        );
    }

    #[test]
    fn every_event_round_trips_through_its_name() {
        for &e in SignalEvent::ALL {
            assert_eq!(SignalEvent::from_name(e.name()), Some(e));
        }
    }
}
