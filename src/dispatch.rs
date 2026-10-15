//! The single event loop: consumes [`Event`]s, owns the [`State`] and the
//! active mode, and hands actions to the [`Executor`].
//!
//! Running everything through one loop means hotkey handling and state
//! updates cannot race each other.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use anyhow::Result;
use tracing::{debug, error, info, warn};

use crate::actions::{Action, Executor};
use crate::config::{Config, DEFAULT_MODE};
use crate::events::Event;
use crate::keys::tap::GrabSet;
use crate::spaces;
use crate::state::State;
use crate::yabai::Client;

pub struct Dispatcher {
    rx: Receiver<Event>,
    config_path: PathBuf,
    config: Config,
    mode: String,
    grab: GrabSet,
    client: Client,
    executor: Executor,
    state: State,
}

impl Dispatcher {
    pub fn new(
        rx: Receiver<Event>,
        config_path: PathBuf,
        config: Config,
        grab: GrabSet,
        client: Client,
        executor: Executor,
        state: State,
    ) -> Dispatcher {
        let mut d = Dispatcher {
            rx,
            config_path,
            config,
            mode: DEFAULT_MODE.to_string(),
            grab,
            client,
            executor,
            state,
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
            other => {
                debug!(%hotkey, ?other, "dispatching");
                self.executor.submit(other);
            }
        }
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

    fn reload(&mut self) {
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
