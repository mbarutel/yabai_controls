//! Typed wrappers over `yabai -m query --*`.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

use super::Client;
use super::model::{Display, Space, SpaceId, Window, WindowId};

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

    pub fn focused_space(&self) -> Result<Space> {
        self.query(&["query", "--spaces", "--space"])
    }

    pub fn focused_window(&self) -> Result<Window> {
        self.query(&["query", "--windows", "--window"])
    }

    pub fn window(&self, id: WindowId) -> Result<Window> {
        self.query(&["query", "--windows", "--window", &id.to_string()])
    }

    pub fn space(&self, id: SpaceId) -> Result<Space> {
        self.query(&["query", "--spaces", "--space", &id.to_string()])
    }

    /// Windows on the given space index.
    pub fn windows_on_space(&self, space_index: u32) -> Result<Vec<Window>> {
        self.query(&["query", "--windows", "--space", &space_index.to_string()])
    }

    fn query<T: DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        let body = self.send(args)?;
        serde_json::from_str(&body).with_context(|| format!("parsing reply to {}", args.join(" ")))
    }
}
