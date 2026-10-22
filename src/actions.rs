use std::process::Command;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};

use anyhow::{Result, bail};
use tracing::{debug, error, info};

use crate::yabai::Client;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Yabai(Vec<String>),
    Shell(String),
    Mode(String),
    Focus(FocusStep),
    /// Focus a window of this app wherever it is, cycling its windows on
    /// repeat; with `launch`, `open -a` it when it has none. Resolved by
    /// `dispatch` against `State`; never reaches the worker.
    App {
        name: String,
        launch: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusStep {
    PreviousWindow,
    PreviousSpace,
    Clockwise,
    CounterClockwise,
}

impl FocusStep {
    pub const NAMES: [&str; 4] = [
        "previous-window",
        "previous-space",
        "clockwise",
        "counter-clockwise",
    ];

    pub fn from_name(name: &str) -> Option<FocusStep> {
        match name {
            "previous-window" => Some(FocusStep::PreviousWindow),
            "previous-space" => Some(FocusStep::PreviousSpace),
            "clockwise" => Some(FocusStep::Clockwise),
            "counter-clockwise" => Some(FocusStep::CounterClockwise),
            _ => None,
        }
    }
}

impl Action {
    /// Parses a `yabai -m`-style argument string with basic quote handling,
    /// so `window --label 'my window'` becomes three arguments.
    pub fn yabai_from_str(s: &str) -> Result<Action> {
        let args = split_args(s)?;
        if args.is_empty() {
            bail!("empty yabai command");
        }
        Ok(Action::Yabai(args))
    }
}

/// Handle to the worker thread. Dropping it closes the queue; call `join`
/// to wait for in-flight actions.
pub struct Executor {
    tx: Sender<Action>,
    handle: Option<JoinHandle<()>>,
}

impl Executor {
    pub fn spawn(client: Client, shell: String) -> Executor {
        let (tx, rx) = channel();
        let handle = thread::Builder::new()
            .name("actions".into())
            .spawn(move || worker(rx, client, shell))
            .expect("failed to spawn action worker");
        Executor {
            tx,
            handle: Some(handle),
        }
    }

    pub fn submit(&self, action: Action) {
        if self.tx.send(action).is_err() {
            error!("action worker has exited");
        }
    }

    pub fn join(mut self) {
        drop(std::mem::replace(&mut self.tx, channel().0));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn worker(rx: Receiver<Action>, client: Client, shell: String) {
    info!("action worker started");
    for action in rx {
        if let Err(err) = run(&action, &client, &shell) {
            error!(?action, %err, "action failed");
        }
    }
    info!("action worker stopped");
}

fn run(action: &Action, client: &Client, shell: &str) -> Result<()> {
    match action {
        Action::Yabai(args) => {
            debug!(?args, "yabai");
            let reply = client.send(args)?;
            if !reply.is_empty() {
                debug!(reply = reply.trim_end(), "yabai reply");
            }
        }
        Action::Shell(cmd) => {
            debug!(%cmd, "shell");
            let status = Command::new(shell).arg("-c").arg(cmd).status()?;
            if !status.success() {
                bail!("shell command exited with {status}");
            }
        }
        Action::Mode(name) => {
            bail!("mode switch to '{name}' reached the action worker; dispatch should handle it")
        }
        Action::Focus(step) => {
            bail!("focus step {step:?} reached the action worker; dispatch should resolve it")
        }
        Action::App { name, .. } => {
            bail!("app focus '{name}' reached the action worker; dispatch should resolve it")
        }
    }
    Ok(())
}

/// Splits on whitespace, honouring single quotes, double quotes and
/// backslash escapes. Enough for yabai arguments; not a full shell parser.
fn split_args(s: &str) -> Result<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_arg = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();

    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                let next = chars
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("trailing backslash"))?;
                current.push(next);
            }
            (Some(_), c) => current.push(c),
            (None, '\'') | (None, '"') => {
                quote = Some(c);
                in_arg = true;
            }
            (None, '\\') => {
                let next = chars
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("trailing backslash"))?;
                current.push(next);
                in_arg = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_arg {
                    args.push(std::mem::take(&mut current));
                    in_arg = false;
                }
            }
            (None, c) => {
                current.push(c);
                in_arg = true;
            }
        }
    }

    if quote.is_some() {
        bail!("unterminated quote in '{s}'");
    }
    if in_arg {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_plain_arguments() {
        assert_eq!(
            split_args("space  --focus 3").unwrap(),
            vec!["space", "--focus", "3"]
        );
    }

    #[test]
    fn keeps_quoted_whitespace_together() {
        assert_eq!(
            split_args(r#"window --label 'my window' --title "a \"b\"""#).unwrap(),
            vec!["window", "--label", "my window", "--title", "a \"b\""]
        );
    }

    #[test]
    fn empty_quotes_produce_an_empty_argument() {
        assert_eq!(
            split_args("signal --add label=''").unwrap(),
            vec!["signal", "--add", "label="]
        );
        assert_eq!(split_args("a '' b").unwrap(), vec!["a", "", "b"]);
    }

    #[test]
    fn rejects_unterminated_quote() {
        assert!(split_args("window --title 'oops").is_err());
    }
}
