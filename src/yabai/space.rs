//! `yabai -m space` mutation wrappers. Queries live in `query.rs`.

use anyhow::Result;

use super::Client;

impl Client {
    /// Creates a new space on the focused display. Requires yabai's
    /// scripting addition.
    pub fn create_space(&self) -> Result<()> {
        self.send(&["space", "--create"])?;
        Ok(())
    }

    /// Sets a space's label. yabai cannot clear a label; it can only be
    /// replaced.
    pub fn label_space(&self, index: u32, label: &str) -> Result<()> {
        self.send(&["space", &index.to_string(), "--label", label])?;
        Ok(())
    }
}
