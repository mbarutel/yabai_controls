//! What bindings do ([`Action`]), what the worker thread runs ([`Command`]),
//! and the worker itself ([`Executor`]).

use std::process;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};

use anyhow::{Result, bail};
use tracing::{debug, error, info};

use crate::yabai::Client;

/// What a binding does. `Command`s go straight to the worker; the other
/// variants are resolved by `dispatch` against its mode and `State` and
/// become zero or more `Command`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Command(Command),
    Mode(String),
    Focus(FocusStep),
    /// Focus a window of this app wherever it is, cycling its windows on
    /// repeat; with `launch`, `open -a` it when it has none.
    App {
        name: String,
        launch: bool,
    },
}

/// Something the worker thread can run on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `yabai -m <args>`.
    Yabai(Vec<String>),
    /// `$SHELL -c <cmd>`.
    Shell(String),
}

impl Command {
    /// `Command::yabai(&["space", "--focus", "3"])`.
    pub fn yabai<S: AsRef<str>>(args: &[S]) -> Command {
        Command::Yabai(args.iter().map(|a| a.as_ref().to_string()).collect())
    }
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

impl Command {
    /// Parses a `yabai -m`-style argument string with basic quote handling,
    /// so `window --label 'my window'` becomes three arguments.
    pub fn yabai_from_str(s: &str) -> Result<Command> {
        let args = split_args(s)?;
        if args.is_empty() {
            bail!("empty yabai command");
        }
        Ok(Command::Yabai(args))
    }
}

/// Handle to the worker thread. Dropping it closes the queue; call `join`
/// to wait for in-flight commands.
pub struct Executor {
    tx: Sender<Command>,
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

    pub fn submit(&self, command: Command) {
        if self.tx.send(command).is_err() {
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

fn worker(rx: Receiver<Command>, client: Client, shell: String) {
    info!("action worker started");
    for command in rx {
        if let Err(err) = run(&command, &client, &shell) {
            error!(?command, %err, "command failed");
        }
    }
    info!("action worker stopped");
}

fn run(command: &Command, client: &Client, shell: &str) -> Result<()> {
    match command {
        Command::Yabai(args) => {
            debug!(?args, "yabai");
            let reply = client.send(args)?;
            if !reply.is_empty() {
                debug!(reply = reply.trim_end(), "yabai reply");
            }
        }
        Command::Shell(cmd) => {
            debug!(%cmd, "shell");
            let status = process::Command::new(shell).arg("-c").arg(cmd).status()?;
            if !status.success() {
                bail!("shell command exited with {status}");
            }
        }
    }
    Ok(())
}

/// Quotes `s` as one `sh` word. Plain path-like strings pass through
/// unchanged so logged commands stay readable; anything else is
/// single-quoted, so `open -a 'Google Chrome'` survives spaces and `'`.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
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

    #[test]
    fn shell_quote_passes_plain_words_and_quotes_the_rest() {
        assert_eq!(
            shell_quote("/usr/local/bin/yabai_controls"),
            "/usr/local/bin/yabai_controls"
        );
        assert_eq!(shell_quote("Google Chrome"), "'Google Chrome'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
        // Round-trips through our own splitter.
        assert_eq!(split_args(&shell_quote("it's x")).unwrap(), vec!["it's x"]);
    }
}
