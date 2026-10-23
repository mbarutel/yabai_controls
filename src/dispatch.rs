//! The single event loop: consumes [`Event`]s, owns the [`State`] and the
//! active mode, and hands actions to the [`Executor`].
//!
//! Running everything through one loop means hotkey handling and state
//! updates cannot race each other.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use crate::actions::{Action, Command, Executor, FocusStep, shell_quote};
use crate::config::{Config, DEFAULT_MODE};
use crate::events::Event;
use crate::keys::Hotkey;
use crate::keys::tap::GrabSet;
use crate::ring::{self, Direction};
use crate::spaces;
use crate::state::State;
use crate::yabai::Client;
use crate::yabai::model::{Space, SpaceId, WindowId};
use crate::yabai::signal::SignalEvent;

/// Display add/remove and wake signals arrive in bursts; `[spaces]` is
/// re-synced at most once per burst.
const DISPLAY_SYNC_DEBOUNCE: Duration = Duration::from_secs(2);

pub struct Dispatcher {
    rx: Receiver<Event>,
    config_path: PathBuf,
    config: Config,
    mode: String,
    grab: GrabSet,
    client: Client,
    executor: Executor,
    state: State,
    /// Path yabai should run for `notify`; `None` when signals are disabled.
    notify_bin: Option<String>,
    last_display_sync: Option<Instant>,
}

impl Dispatcher {
    pub fn new(
        rx: Receiver<Event>,
        config_path: PathBuf,
        config: Config,
        grab: GrabSet,
        client: Client,
        state: State,
        notify_bin: Option<String>,
    ) -> Dispatcher {
        let executor = Executor::spawn(client.clone(), config.shell.clone());
        let mut d = Dispatcher {
            rx,
            config_path,
            config,
            mode: DEFAULT_MODE.to_string(),
            grab,
            client,
            executor,
            state,
            notify_bin,
            last_display_sync: None,
        };
        d.enter_mode(DEFAULT_MODE);
        d
    }

    /// Runs until a `Shutdown` event arrives or every sender is dropped.
    pub fn run(mut self) -> Executor {
        info!("dispatch loop started");
        while let Ok(event) = self.rx.recv() {
            match event {
                Event::Hotkey(hotkey) => self.on_hotkey(hotkey),
                Event::Yabai { event, args } => self.on_yabai(event, &args),
                Event::Reload => self.reload(),
                Event::Shutdown => break,
            }
        }
        info!("dispatch loop stopped");

        spaces::remove_all_rules(&self.client);
        self.executor
    }

    /// Keeps `[spaces]` in yabai and the snapshot current. Space work runs
    /// before the state refresh so the snapshot sees the new label and
    /// moved windows.
    fn on_yabai(&mut self, event: SignalEvent, args: &[String]) {
        if event == SignalEvent::SpaceCreated
            && let Err(err) = spaces::adopt_new_spaces(&self.client, &self.config.spaces)
        {
            warn!(%err, "adopting assignments for new space failed");
        }

        let display_changed = matches!(
            event,
            SignalEvent::DisplayAdded | SignalEvent::DisplayRemoved | SignalEvent::SystemWoke
        );
        if display_changed
            && self
                .last_display_sync
                .is_none_or(|t| t.elapsed() > DISPLAY_SYNC_DEBOUNCE)
        {
            self.last_display_sync = Some(Instant::now());
            if let Err(err) = spaces::sync(&self.client, &self.config.spaces) {
                warn!(?event, %err, "re-syncing space assignments failed");
            }
        }

        if let Err(err) = self.state.apply(&self.client, event, args) {
            warn!(?event, %err, "failed to apply yabai event to state");
        }
    }

    fn on_hotkey(&mut self, hotkey: Hotkey) {
        let Some(action) = self
            .config
            .bindings(&self.mode)
            .and_then(|b| b.get(&hotkey))
            .cloned()
        else {
            // Grab set and bindings can briefly disagree during a reload.
            debug!(%hotkey, mode = %self.mode, "no binding");
            return;
        };

        match action {
            Action::Mode(target) => self.enter_mode(&target),
            Action::Focus(step) => self.focus_step(step),
            Action::App { name, launch } => self.focus_app(&name, launch),
            Action::Command(command) => {
                debug!(%hotkey, ?command, "dispatching");
                self.executor.submit(command);
            }
        }
    }

    /// Resolves a focus step against the current state into the yabai
    /// command that performs it. For the previous-* steps the current and
    /// previous slots swap now; the `window_focused` / `space_changed`
    /// report it causes then matches the current slot and changes nothing
    /// (see `focus::LastFocus`).
    fn focus_step(&mut self, step: FocusStep) {
        use FocusStep::*;
        // Windows can close without a signal reaching us in time.
        self.state.prune_focus();
        let target = match step {
            PreviousWindow => self.state.window_focus.back().map(Target::Window),
            PreviousSpace => self.state.space_focus.back().map(Target::Space),
            Clockwise => self.around_space(Direction::Clockwise).map(Target::Window),
            CounterClockwise => self
                .around_space(Direction::CounterClockwise)
                .map(Target::Window),
        };
        match target {
            Some(Target::Window(id)) => {
                debug!(?step, id, "focus step");
                self.focus_window(id);
            }
            Some(Target::Space(id)) => match self.state.space_by_id(id) {
                Some(space) => {
                    debug!(?step, index = space.index, "focus step");
                    self.executor.submit(space_focus(space.index));
                }
                None => debug!(?step, id, "previous space is gone"),
            },
            None => debug!(?step, "nothing to focus in that direction"),
        }
    }

    /// Focuses a window, switching to its space first when that space is
    /// hidden. yabai's `window --focus` only makes the window frontmost and
    /// lets macOS follow it to its space, which plays the slide animation
    /// when the space is on the current display; `space --focus` goes through
    /// the scripting addition and switches instantly, after which the window
    /// focus happens in place.
    fn focus_window(&self, id: WindowId) {
        let space = self.state.space_of_window(id);
        for command in focus_window_commands(space, id) {
            self.executor.submit(command);
        }
    }

    /// Focuses a window of `app` wherever it is; see `State::pick_app_window`
    /// for which one. Reads the snapshot rather than querying: it is kept
    /// current by the window signals and holds the recency a query cannot
    /// give.
    fn focus_app(&mut self, app: &str, launch: bool) {
        self.state.prune_focus();
        match self.state.pick_app_window(app) {
            Some(id) => {
                debug!(app, id, "focus app");
                self.focus_window(id);
            }
            None if launch => {
                debug!(app, "no window; launching");
                self.executor
                    .submit(Command::Shell(format!("open -a {}", shell_quote(app))));
            }
            None => debug!(app, "no window to focus"),
        }
    }

    /// Neighbour of the focused window around the focused space. Queries
    /// yabai rather than the snapshot so a drag or resize that has not yet
    /// been signalled cannot send focus the wrong way.
    fn around_space(&self, direction: Direction) -> Option<WindowId> {
        let windows = match self.client.windows_on_focused_space() {
            Ok(w) => w,
            Err(err) => {
                warn!(%err, "querying focused space windows");
                return None;
            }
        };
        let current = windows.iter().find(|w| w.has_focus).map(|w| w.id);
        ring::neighbour(&ring::ring(&windows), current, direction)
    }

    fn enter_mode(&mut self, name: &str) {
        let Some(bindings) = self.config.bindings(name) else {
            error!(mode = name, "unknown mode; staying in '{}'", self.mode);
            return;
        };
        self.grab.replace(bindings.keys().copied());
        if self.mode != name {
            info!(from = %self.mode, to = name, "mode changed");
        }
        self.mode = name.to_string();
    }

    /// Re-reads the config and re-establishes everything the daemon keeps
    /// in yabai: `[spaces]` rules and the signal subscriptions. yabai holds
    /// both only in memory, so `.yabairc` should end with
    /// `yabai_controls reload` to heal after a yabai restart.
    fn reload(&mut self) {
        if let Some(bin) = &self.notify_bin {
            match self.client.register_signals(SignalEvent::DEFAULT, bin) {
                Ok(()) => info!(
                    count = SignalEvent::DEFAULT.len(),
                    "yabai signals registered"
                ),
                Err(err) => error!(%err, "re-registering yabai signals failed"),
            }
        }
        match Config::load(&self.config_path) {
            Ok(config) => {
                if let Err(err) = spaces::sync(&self.client, &config.spaces) {
                    error!(%err, "space assignments failed; continuing with new bindings");
                }
                self.config = config;
                info!(path = %self.config_path.display(), "config reloaded");
                self.enter_mode(DEFAULT_MODE);
            }
            Err(err) => error!(%err, "config reload failed; keeping previous config"),
        }
        if let Err(err) = self.state.refresh_all(&self.client) {
            warn!(%err, "state refresh after reload failed");
        }
    }
}

/// What a focus step resolved to.
enum Target {
    Window(WindowId),
    Space(SpaceId),
}

fn space_focus(index: u32) -> Command {
    Command::yabai(&["space", "--focus", &index.to_string()])
}

/// yabai commands that focus window `id`, given the space the snapshot
/// says it is on: a `space --focus` first when that space is hidden, then
/// the `window --focus`. An unknown space (window not in the snapshot) just
/// focuses the window and lets macOS switch.
fn focus_window_commands(space: Option<&Space>, id: WindowId) -> Vec<Command> {
    let mut commands = Vec::with_capacity(2);
    if let Some(space) = space.filter(|s| !s.is_visible) {
        commands.push(space_focus(space.index));
    }
    commands.push(Command::yabai(&["window", "--focus", &id.to_string()]));
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(index: u32, is_visible: bool) -> Space {
        Space {
            index,
            is_visible,
            ..Default::default()
        }
    }

    #[test]
    fn hidden_space_is_focused_before_the_window() {
        assert_eq!(
            focus_window_commands(Some(&space(3, false)), 42),
            vec![
                Command::yabai(&["space", "--focus", "3"]),
                Command::yabai(&["window", "--focus", "42"]),
            ]
        );
    }

    #[test]
    fn visible_or_unknown_space_focuses_the_window_only() {
        let only_window = vec![Command::yabai(&["window", "--focus", "42"])];
        assert_eq!(
            focus_window_commands(Some(&space(3, true)), 42),
            only_window
        );
        assert_eq!(focus_window_commands(None, 42), only_window);
    }
}
