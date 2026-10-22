//! The single event loop: consumes [`Event`]s, owns the [`State`] and the
//! active mode, and hands actions to the [`Executor`].
//!
//! Running everything through one loop means hotkey handling and state
//! updates cannot race each other.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{debug, error, info, warn};

use crate::actions::{Action, Executor, FocusStep};
use crate::config::{Config, DEFAULT_MODE};
use crate::events::Event;
use crate::keys::tap::GrabSet;
use crate::ring::{self, Direction};
use crate::spaces;
use crate::state::State;
use crate::yabai::Client;
use crate::yabai::model::{SpaceId, WindowId};
use crate::yabai::signal::SignalEvent;

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
                Event::Yabai { event, args } => {
                    // Before the state refresh so the snapshot sees the new
                    // label and moved windows.
                    if event == SignalEvent::SpaceCreated
                        && let Err(err) =
                            spaces::adopt_new_spaces(&self.client, &self.config.spaces)
                    {
                        warn!(%err, "adopting assignments for new space failed");
                    }

                    if matches!(
                        event,
                        SignalEvent::DisplayAdded
                            | SignalEvent::DisplayRemoved
                            | SignalEvent::SystemWoke
                    ) && self
                        .last_display_sync
                        .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
                    {
                        self.last_display_sync = Some(Instant::now());
                        if let Err(err) = spaces::sync(&self.client, &self.config.spaces) {
                            warn!(?event, %err, "re-syncing space assignments failed");
                        }
                    }

                    if let Err(err) = self.state.apply(&self.client, event, &args) {
                        warn!(?event, %err, "failed to apply yabai event to state");
                    }
                }
                Event::Reload => self.reload(),
                Event::Shutdown => break,
            }
        }
        info!("dispatch loop stopped");

        spaces::remove_all_rules(&self.client);
        self.executor
    }

    fn on_hotkey(&mut self, hotkey: crate::keys::Hotkey) {
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
            other => {
                debug!(%hotkey, ?other, "dispatching");
                self.executor.submit(other);
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
        let window = |id: WindowId| vec!["window".to_string(), "--focus".into(), id.to_string()];
        let space = |id: SpaceId| {
            self.state
                .spaces
                .iter()
                .find(|s| s.id == id)
                .map(|s| vec!["space".to_string(), "--focus".into(), s.index.to_string()])
        };
        let args = match step {
            PreviousWindow => self.state.window_focus.back().map(window),
            PreviousSpace => self.state.space_focus.back().and_then(space),
            Clockwise => self.around_space(Direction::Clockwise).map(window),
            CounterClockwise => self.around_space(Direction::CounterClockwise).map(window),
        };
        match args {
            Some(args) => {
                debug!(?step, ?args, "focus step");
                self.executor.submit(Action::Yabai(args));
            }
            None => debug!(?step, "nothing to focus in that direction"),
        }
    }

    /// Focuses a window of `app` wherever it is (yabai switches space and
    /// display as needed); see `State::pick_app_window` for which one.
    /// Reads the snapshot rather than querying: it is kept current by the
    /// window signals and holds the recency a query cannot give.
    fn focus_app(&mut self, app: &str, launch: bool) {
        self.state.prune_focus();
        match self.state.pick_app_window(app) {
            Some(id) => {
                debug!(app, id, "focus app");
                self.executor.submit(Action::Yabai(vec![
                    "window".into(),
                    "--focus".into(),
                    id.to_string(),
                ]));
            }
            None if launch => {
                debug!(app, "no window; launching");
                self.executor
                    .submit(Action::Shell(format!("open -a {}", shell_quote(app))));
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

/// Single-quotes `s` for `sh -c`, so `open -a 'Google Chrome'` survives
/// spaces and `'` in app names.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Convenience for the `state` subcommand: fetch once and print a summary.
pub fn print_state(client: &Client) -> Result<()> {
    let state = State::fetch(client)?;
    for display in &state.displays {
        println!(
            "display {} ({}x{}){}",
            display.index,
            display.frame.w,
            display.frame.h,
            if display.has_focus { " *" } else { "" }
        );
        for space_index in &display.spaces {
            let Some(space) = state.space_by_index(*space_index) else {
                continue;
            };
            println!(
                "  space {} [{:?}]{}{}",
                space.index,
                space.layout,
                if space.has_focus { " *" } else { "" },
                if space.label.is_empty() {
                    String::new()
                } else {
                    format!(" \"{}\"", space.label)
                }
            );
            for window in state.windows_on_space(space.index) {
                println!(
                    "    {:>6} {:<20} {}{}",
                    window.id,
                    window.app,
                    window.title,
                    if window.has_focus { " *" } else { "" }
                );
            }
        }
    }
    Ok(())
}
