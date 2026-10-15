//! The daemon's inbound event type and its own control socket.
//!
//! Everything the dispatch loop reacts to arrives as an [`Event`]: hotkeys
//! from the tap, yabai signals forwarded by `yabai_controls notify`, and
//! control messages (`reload`, `shutdown`) from the CLI.
//!
//! Socket protocol: one UTF-8 line per connection, whitespace separated:
//!
//! ```text
//! notify <event_name> [arg...]
//! reload
//! shutdown
//! ```

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow, bail};
use tracing::{debug, error, info, warn};

use crate::keys::Hotkey;
use crate::yabai::signal::SignalEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Hotkey(Hotkey),
    Yabai {
        event: SignalEvent,
        args: Vec<String>,
    },
    Reload,
    Shutdown,
}

impl Event {
    /// Parses one control-socket line.
    pub fn parse_line(line: &str) -> Result<Event> {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("notify") => {
                let name = words
                    .next()
                    .ok_or_else(|| anyhow!("notify: missing event name"))?;
                let event = SignalEvent::from_name(name)
                    .ok_or_else(|| anyhow!("notify: unknown event '{name}'"))?;
                Ok(Event::Yabai {
                    event,
                    args: words.map(str::to_string).collect(),
                })
            }
            Some("reload") => Ok(Event::Reload),
            Some("shutdown") => Ok(Event::Shutdown),
            Some(other) => bail!("unknown command '{other}'"),
            None => bail!("empty command"),
        }
    }
}

pub fn control_socket_path_for_user(user: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/yabai_controls_{user}.socket"))
}

pub fn control_socket_path() -> Result<PathBuf> {
    let user = std::env::var("USER").context("USER is not set")?;
    Ok(control_socket_path_for_user(&user))
}

/// Listens on the control socket and forwards parsed events to `tx`.
/// Returns once the listener is bound; the accept loop runs on its own thread.
pub fn listen(path: &Path, tx: Sender<Event>) -> Result<Listener> {
    if path.exists() {
        // Stale socket from a previous run, or another instance. Probe it.
        if UnixStream::connect(path).is_ok() {
            bail!(
                "another yabai_controls daemon is listening on {}",
                path.display()
            );
        }
        std::fs::remove_file(path)
            .with_context(|| format!("removing stale socket {}", path.display()))?;
    }

    let listener = UnixListener::bind(path)
        .with_context(|| format!("binding control socket {}", path.display()))?;
    info!(path = %path.display(), "control socket listening");

    let handle = thread::Builder::new()
        .name("control-socket".into())
        .spawn(move || accept_loop(listener, tx))
        .expect("failed to spawn control socket thread");

    Ok(Listener {
        path: path.to_path_buf(),
        _handle: handle,
    })
}

/// Removes the socket file on drop.
pub struct Listener {
    path: PathBuf,
    _handle: JoinHandle<()>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn accept_loop(listener: UnixListener, tx: Sender<Event>) {
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(err) => {
                error!(%err, "accept failed");
                continue;
            }
        };
        let mut line = String::new();
        if let Err(err) = BufReader::new(&stream).read_line(&mut line) {
            warn!(%err, "reading control message");
            continue;
        }
        match Event::parse_line(&line) {
            Ok(event) => {
                debug!(?event, "control message");
                let stop = event == Event::Shutdown;
                if tx.send(event).is_err() {
                    info!("dispatch gone; control socket stopping");
                    return;
                }
                if stop {
                    return;
                }
            }
            Err(err) => {
                warn!(%err, line = line.trim_end(), "bad control message");
                let mut stream = stream;
                let _ = writeln!(stream, "error: {err}");
            }
        }
    }
}

/// Sends one control line to a running daemon.
pub fn send_control(path: &Path, line: &str) -> Result<()> {
    let mut stream = UnixStream::connect(path)
        .with_context(|| format!("connecting to {} (is the daemon running?)", path.display()))?;
    writeln!(stream, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notify_with_args() {
        let ev = Event::parse_line("notify space_changed 3 2 1 1\n").unwrap();
        assert_eq!(
            ev,
            Event::Yabai {
                event: SignalEvent::SpaceChanged,
                args: vec!["3".into(), "2".into(), "1".into(), "1".into()]
            }
        );
    }

    #[test]
    fn parses_control_words() {
        assert_eq!(Event::parse_line("reload").unwrap(), Event::Reload);
        assert_eq!(Event::parse_line("shutdown\n").unwrap(), Event::Shutdown);
    }

    #[test]
    fn rejects_unknown_event_and_command() {
        assert!(Event::parse_line("notify not_an_event").is_err());
        assert!(Event::parse_line("dance").is_err());
        assert!(Event::parse_line("").is_err());
    }
}
