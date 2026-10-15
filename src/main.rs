//! CLI entry point. See `lib.rs` for the module map and thread layout.

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::thread;

use anyhow::{Context, Result, bail};
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

const USAGE: &str = "\
usage: yabai_controls <command> [options]

commands:
  daemon [--config PATH] [--no-signals]   run the hotkey daemon (needs Accessibility permission)
  notify <event> [args...]                forward a yabai signal to the running daemon
  reload                                  ask the running daemon to reload its config
  shutdown                                stop the running daemon
  state                                   print displays/spaces/windows from yabai and exit
  check-config [--config PATH]            parse the config file and report errors

environment:
  RUST_LOG   log filter, e.g. RUST_LOG=debug (default: info)
";

enum Command {
    Daemon { config: PathBuf, signals: bool },
    Notify { line: String },
    Reload,
    Shutdown,
    State,
    CheckConfig { config: PathBuf },
}

fn parse_args(args: &[String]) -> Result<Command> {
    let mut it = args.iter().skip(1);
    let cmd = it.next().map(String::as_str).unwrap_or("");
    let rest: Vec<&String> = it.collect();

    let config_flag = |rest: &[&String]| -> Result<PathBuf> {
        let mut config = Config::default_path();
        let mut i = 0;
        while i < rest.len() {
            match rest[i].as_str() {
                "--config" | "-c" => {
                    config = PathBuf::from(rest.get(i + 1).context("--config requires a path")?);
                    i += 2;
                }
                "--no-signals" => i += 1,
                other => bail!("unknown option '{other}'\n\n{USAGE}"),
            }
        }
        Ok(config)
    };

    Ok(match cmd {
        "daemon" => Command::Daemon {
            config: config_flag(&rest)?,
            signals: !rest.iter().any(|a| *a == "--no-signals"),
        },
        "notify" => {
            if rest.is_empty() {
                bail!("notify requires an event name\n\n{USAGE}");
            }
            let words: Vec<&str> = rest.iter().map(|s| s.as_str()).collect();
            Command::Notify {
                line: format!("notify {}", words.join(" ")),
            }
        }
        "reload" => Command::Reload,
        "shutdown" => Command::Shutdown,
        "state" => Command::State,
        "check-config" => Command::CheckConfig {
            config: config_flag(&rest)?,
        },
        _ => bail!("{USAGE}"),
    })
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();
    if let Err(err) = parse_args(&args).and_then(run) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::Daemon { config, signals } => run_daemon(config, signals),
        Command::Notify { line } => events::send_control(&events::control_socket_path()?, &line),
        Command::Reload => events::send_control(&events::control_socket_path()?, "reload"),
        Command::Shutdown => events::send_control(&events::control_socket_path()?, "shutdown"),
        Command::State => dispatch::print_state(&Client::from_env()?),
        Command::CheckConfig { config } => {
            let cfg = Config::load(&config)?;
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
