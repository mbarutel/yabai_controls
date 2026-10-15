//! Serde models for `yabai -m query --*` replies.
//!
//! Field names in yabai's JSON are kebab-case. Fields are `#[serde(default)]`
//! so a yabai version that adds or drops a field does not break parsing.

use serde::Deserialize;

pub type DisplayId = u32;
pub type SpaceId = u64;
pub type WindowId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Display {
    pub id: DisplayId,
    pub uuid: String,
    pub index: u32,
    pub label: String,
    pub frame: Frame,
    /// Space indices on this display.
    pub spaces: Vec<u32>,
    pub has_focus: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpaceLayout {
    #[default]
    Bsp,
    Stack,
    Float,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Space {
    pub id: SpaceId,
    pub uuid: String,
    pub index: u32,
    pub label: String,
    #[serde(rename = "type")]
    pub layout: SpaceLayout,
    /// Display index this space belongs to.
    pub display: u32,
    pub windows: Vec<WindowId>,
    pub first_window: WindowId,
    pub last_window: WindowId,
    pub has_focus: bool,
    pub is_visible: bool,
    pub is_native_fullscreen: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Window {
    pub id: WindowId,
    pub pid: i32,
    pub app: String,
    pub title: String,
    pub scratchpad: String,
    pub frame: Frame,
    pub role: String,
    pub subrole: String,
    pub root_window: bool,
    /// Display index.
    pub display: u32,
    /// Space index.
    pub space: u32,
    pub level: i32,
    pub sub_level: i32,
    pub layer: String,
    pub sub_layer: String,
    pub opacity: f64,
    pub split_type: String,
    pub split_child: String,
    pub stack_index: u32,
    pub can_move: bool,
    pub can_resize: bool,
    pub has_focus: bool,
    pub has_shadow: bool,
    pub has_parent_zoom: bool,
    pub has_fullscreen_zoom: bool,
    pub has_ax_reference: bool,
    pub is_native_fullscreen: bool,
    pub is_visible: bool,
    pub is_minimized: bool,
    pub is_hidden: bool,
    pub is_floating: bool,
    pub is_sticky: bool,
    pub is_grabbed: bool,
}

impl Window {
    /// True for ordinary application windows, which is what most focus /
    /// swap commands should operate on.
    pub fn is_standard(&self) -> bool {
        self.role == "AXWindow" && self.subrole == "AXStandardWindow"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_window_with_kebab_case_fields() {
        let json = r#"{
            "id": 793, "pid": 3545, "app": "Ghostty", "title": "x",
            "frame": {"x": 10.0, "y": 48.0, "w": 2036.0, "h": 1271.0},
            "role": "AXWindow", "subrole": "AXStandardWindow",
            "display": 1, "space": 3, "stack-index": 0,
            "has-focus": true, "is-native-fullscreen": false,
            "some-field-from-the-future": 42
        }"#;
        let w: Window = serde_json::from_str(json).unwrap();
        assert_eq!(w.id, 793);
        assert_eq!(w.space, 3);
        assert!(w.has_focus);
        assert!(w.is_standard());
        assert_eq!(w.frame.w, 2036.0);
    }

    #[test]
    fn parses_space_type_into_layout() {
        let json = r#"[{"id": 3, "index": 2, "type": "stack", "display": 1, "windows": [938]}]"#;
        let spaces: Vec<Space> = serde_json::from_str(json).unwrap();
        assert_eq!(spaces[0].layout, SpaceLayout::Stack);
        assert_eq!(spaces[0].windows, vec![938]);
    }
}
