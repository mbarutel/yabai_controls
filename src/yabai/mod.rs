//! Client for the yabai daemon's unix socket.
//!
//! Wire protocol (from yabai's `src/yabai.c` / `src/event_loop.c`):
//!
//! ```text
//! [native-endian i32: payload length]
//! [arg1][\0] [arg2][\0] ... [argN][\0] [\0]
//! ```
//!
//! The daemon replies with raw bytes and closes the stream. An empty reply
//! means success with no output. A reply whose first byte is `0x07` (BEL) is
//! an error message.

pub mod model;
pub mod query;
pub mod signal;

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

const FAILURE_BYTE: u8 = 0x07;

/// Handle to the yabai socket. Cheap to clone; each `send` opens a fresh
/// connection, matching how the `yabai -m` client behaves.
#[derive(Debug, Clone)]
pub struct Client {
    socket_path: PathBuf,
}

impl Client {
    /// Connects to `/tmp/yabai_$USER.socket`.
    pub fn from_env() -> Result<Self> {
        let user = std::env::var("USER").context("USER is not set")?;
        Ok(Self::new(socket_path_for_user(&user)))
    }

    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Sends a command (without the leading `-m`) and returns the reply body.
    ///
    /// `client.send(&["space", "--focus", "3"])` is equivalent to
    /// `yabai -m space --focus 3`.
    pub fn send<S: AsRef<str>>(&self, args: &[S]) -> Result<String> {
        let mut stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!(
                "connecting to yabai socket at {}",
                self.socket_path.display()
            )
        })?;

        stream
            .write_all(&encode_message(args))
            .context("writing yabai request")?;
        stream
            .shutdown(Shutdown::Write)
            .context("half-closing yabai socket")?;

        let mut reply = Vec::new();
        stream
            .read_to_end(&mut reply)
            .context("reading yabai reply")?;

        decode_reply(reply)
    }
}

/// Builds the framed request bytes for the given arguments.
pub fn encode_message<S: AsRef<str>>(args: &[S]) -> Vec<u8> {
    let payload_len: usize = args.iter().map(|a| a.as_ref().len() + 1).sum::<usize>() + 1;
    let mut buf = Vec::with_capacity(4 + payload_len);
    buf.extend_from_slice(&(payload_len as i32).to_ne_bytes());
    for arg in args {
        buf.extend_from_slice(arg.as_ref().as_bytes());
        buf.push(0);
    }
    buf.push(0);
    buf
}

/// Interprets a raw reply, turning a BEL-prefixed body into an error.
pub fn decode_reply(reply: Vec<u8>) -> Result<String> {
    match reply.first() {
        Some(&FAILURE_BYTE) => {
            let msg = String::from_utf8_lossy(&reply[1..]).trim_end().to_string();
            Err(anyhow!("yabai: {msg}"))
        }
        _ => Ok(String::from_utf8(reply).context("yabai reply was not UTF-8")?),
    }
}

pub fn socket_path_for_user(user: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/yabai_{user}.socket"))
}
