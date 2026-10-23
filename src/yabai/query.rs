//! Typed wrappers over `yabai -m query --*`, plus the JSON-decoding `query`
//! helper the other mutation modules build on.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

use super::Client;
use super::model::{Display, Space, Window, WindowId};

impl Client {
    pub fn displays(&self) -> Result<Vec<Display>> {
        self.query(&["query", "--displays"])
    }

    pub fn spaces(&self) -> Result<Vec<Space>> {
        self.query(&["query", "--spaces"])
    }

    pub fn windows(&self) -> Result<Vec<Window>> {
        self.query(&["query", "--windows"])
    }

    pub fn window(&self, id: WindowId) -> Result<Window> {
        self.query(&["query", "--windows", "--window", &id.to_string()])
    }

    /// Windows on the focused space.
    pub fn windows_on_focused_space(&self) -> Result<Vec<Window>> {
        self.query(&["query", "--windows", "--space"])
    }

    /// Sends `args` and parses the JSON reply.
    pub(super) fn query<T: DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        let body = self.send(args)?;
        serde_json::from_str(&body).with_context(|| format!("parsing reply to {}", args.join(" ")))
    }
}
