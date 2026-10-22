//! CLI entry point. See `lib.rs` for the module map and thread layout.

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::thread;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use yabai_controls::config::Config;
use yabai_controls::dispatch::{self, Dispatcher};
use yabai_controls::events::{self, Event};
use yabai_controls::keys::tap::{GrabSet, Shutdown, Tap};
use yabai_controls::os_signal;
use yabai_controls::spaces;
use yabai_controls::state::State;
use yabai_controls::yabai::Client;
use yabai_controls::yabai::signal::SignalEvent;

/// Hotkey daemon for yabai.
#[derive(Parser)]
#[command(
    version,
    arg_required_else_help = true,
    after_help = "environment:\n  RUST_LOG   log filter, e.g. RUST_LOG=debug (default: info)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Shared by `daemon` and `check-config`.
#[derive(Args)]
struct ConfigArg {
    /// Path to config.toml
    #[arg(short, long, value_name = "PATH", default_value_os_t = Config::default_path())]
    config: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// Run the hotkey daemon (needs Accessibility permission)
    Daemon {
        #[command(flatten)]
        config: ConfigArg,
        /// Don't register yabai signals (no `notify` round-trips)
        #[arg(long)]
        no_signals: bool,
    },
    /// Forward a yabai signal to the running daemon (yabai calls this; not for hand use)
    Notify {
        /// Signal name, e.g. space_changed
        event: String,
        /// Signal arguments ($YABAI_* values, may be empty)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Ask the running daemon to reload its config
    Reload,
    /// Stop the running daemon
    Shutdown,
    /// Print displays/spaces/windows from yabai and exit
    State,
    /// Parse the config file and report errors
    CheckConfig {
        #[command(flatten)]
        config: ConfigArg,
    },
}

fn main() {
    // Parse first so --help / --version / usage errors never touch the logger.
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    if let Err(err) = run(cli.command) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::Daemon { config, no_signals } => run_daemon(config.config, !no_signals),
        Command::Notify { event, args } => {
            let mut words = vec![event];
            words.extend(args);
            let line = format!("notify {}", words.join(" "));
            events::send_control(&events::control_socket_path()?, &line)
        }
        Command::Reload => events::send_control(&events::control_socket_path()?, "reload"),
        Command::Shutdown => events::send_control(&events::control_socket_path()?, "shutdown"),
        Command::State => dispatch::print_state(&Client::from_env()?),
        Command::CheckConfig { config } => {
            let cfg = Config::load(&config.config)?;
            let total: usize = cfg.modes.values().map(|m| m.len()).sum();
            println!("ok: {} modes, {} bindings", cfg.modes.len(), total);
            Ok(())
        }
    }
}

fn run_daemon(config_path: PathBuf, register_signals: bool) -> Result<()> {
    let config = Config::load(&config_path)?;
    info!(path = %config_path.display(), modes = config.modes.len(), "config loaded");

    let client = Client::from_env()?;
    client
        .spaces()
        .with_context(|| format!("querying yabai at {}", client.socket_path().display()))?;

    let (tx, rx) = channel::<Event>();
    let grab = GrabSet::new();
    let shutdown = Shutdown::new();

    // Install the tap first: it fails without Accessibility permission, and
    // we want that to happen before anything with side effects (space
    // rules, signals, sockets).
    let tap = {
        let tx = tx.clone();
        Tap::install(grab.clone(), move |hotkey| {
            tx.send(Event::Hotkey(hotkey)).is_ok()
        })?
    };

    let listener = events::listen(&events::control_socket_path()?, tx.clone())?;
    os_signal::forward_termination(tx.clone())?;

    // Apply [spaces] before the first snapshot so State sees the new labels.
    // Only an unreachable yabai is fatal here; skipped spaces are logged.
    spaces::sync(&client, &config.spaces)?;
    let state = State::fetch(&client)?;

    let notify_bin = if register_signals {
        let exe = std::env::current_exe().context("locating own executable")?;
        let exe = exe.to_string_lossy().into_owned();
        client.register_signals(SignalEvent::DEFAULT, &exe)?;
        info!(
            count = SignalEvent::DEFAULT.len(),
            "yabai signals registered"
        );
        Some(exe)
    } else {
        None
    };

    let dispatcher = Dispatcher::new(
        rx,
        config_path,
        config,
        grab.clone(),
        client.clone(),
        state,
        notify_bin,
    );

    let dispatch_thread = {
        let shutdown = shutdown.clone();
        thread::Builder::new()
            .name("dispatch".into())
            .spawn(move || {
                let executor = dispatcher.run();
                shutdown.request();
                executor.join();
            })
            .expect("failed to spawn dispatch thread")
    };

    // Blocks on the main thread until a Shutdown event stops dispatch, which
    // then requests tap shutdown.
    tap.run(shutdown);

    if register_signals {
        client.remove_signals(SignalEvent::DEFAULT);
    }
    drop(tx);
    if dispatch_thread.join().is_err() {
        warn!("dispatch thread panicked");
    }
    drop(listener);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn notify_keeps_event_and_args_in_order() {
        let cli = Cli::try_parse_from(["yabai_controls", "notify", "space_changed", "3", "", "-1"])
            .unwrap();
        match cli.command {
            Command::Notify { event, args } => {
                assert_eq!(event, "space_changed");
                assert_eq!(args, ["3", "", "-1"]);
            }
            _ => panic!("expected notify"),
        }
    }

    #[test]
    fn check_config_rejects_daemon_only_flags() {
        assert!(Cli::try_parse_from(["yabai_controls", "check-config", "--no-signals"]).is_err());
    }
}
