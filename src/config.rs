//! Config file loading: `~/.config/yabai_controls/config.toml`.
//!
//! ```toml
//! [settings]
//! shell = "/bin/zsh"          # optional, defaults to $SHELL or /bin/sh
//!
//! [bindings]                  # the "default" mode
//! "alt - 1" = "space --focus 1"                      # string  => yabai args
//! "alt + shift - return" = { shell = "open -a Ghostty" }
//! "alt - r" = { mode = "resize" }
//!
//! [modes.resize]              # extra modes, entered via { mode = "..." }
//! "h" = "window --resize left:-20:0"
//! "escape" = { mode = "default" }
//!
//! [spaces]                    # dedicate spaces to apps; label defaults to app
//! 1 = { app = "Google Chrome" }
//! 2 = { app = "Microsoft Teams", label = "teams", follow = true }
//! 4 = { label = "scratch" }
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::actions::Action;
use crate::keys::Hotkey;

pub const DEFAULT_MODE: &str = "default";

/// Raw on-disk shape. Keys are chord strings; values are action specs.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    settings: RawSettings,
    #[serde(default)]
    bindings: HashMap<String, RawAction>,
    #[serde(default)]
    modes: HashMap<String, HashMap<String, RawAction>>,
    /// Keyed by space index as written in TOML (`1 = {...}` gives key "1").
    #[serde(default)]
    spaces: HashMap<String, RawSpace>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpace {
    app: Option<String>,
    label: Option<String>,
    #[serde(default)]
    follow: bool,
}

/// A space dedicated to an app and/or given a label.
///
/// This is the config-level fact; everything yabai needs (the rule label,
/// the anchored app regex, the space selector) is derived from it in
/// `crate::spaces`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceAssignment {
    /// 1-based mission-control index, as yabai reports it.
    pub index: u32,
    /// Label applied to the space. Defaults to `app`.
    pub label: String,
    /// Exact application name (`Window::app`) whose windows go to this space.
    pub app: Option<String>,
    /// When true, a new window of `app` also focuses the space.
    pub follow: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSettings {
    shell: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawAction {
    Yabai(String),
    Table {
        yabai: Option<String>,
        shell: Option<String>,
        mode: Option<String>,
    },
}

impl RawAction {
    fn compile(self) -> Result<Action> {
        match self {
            RawAction::Yabai(s) => Action::yabai_from_str(&s),
            RawAction::Table { yabai, shell, mode } => match (yabai, shell, mode) {
                (Some(y), None, None) => Action::yabai_from_str(&y),
                (None, Some(s), None) => Ok(Action::Shell(s)),
                (None, None, Some(m)) => Ok(Action::Mode(m)),
                (None, None, None) => bail!("action needs one of `yabai`, `shell` or `mode`"),
                _ => bail!("action must set exactly one of `yabai`, `shell` or `mode`"),
            },
        }
    }
}

/// Validated, ready-to-use configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub shell: String,
    /// Mode name → (hotkey → action). Always contains `DEFAULT_MODE`.
    pub modes: HashMap<String, HashMap<Hotkey, Action>>,
    /// Sorted by `index`; indices, apps and labels are each unique.
    pub spaces: Vec<SpaceAssignment>,
}

impl Config {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("yabai_controls").join("config.toml")
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing config {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Config> {
        let raw: RawConfig = toml::from_str(text)?;

        let shell = raw
            .settings
            .shell
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/sh".to_string());

        let mut modes = HashMap::new();
        modes.insert(
            DEFAULT_MODE.to_string(),
            compile_bindings(DEFAULT_MODE, raw.bindings)?,
        );
        for (name, bindings) in raw.modes {
            if name == DEFAULT_MODE {
                bail!("use [bindings] for the default mode, not [modes.{DEFAULT_MODE}]");
            }
            modes.insert(name.clone(), compile_bindings(&name, bindings)?);
        }

        // Every `mode = "x"` target must exist.
        for (mode, bindings) in &modes {
            for (hotkey, action) in bindings {
                if let Action::Mode(target) = action
                    && !modes.contains_key(target)
                {
                    bail!(
                        "binding '{hotkey}' in mode '{mode}' switches to unknown mode '{target}'"
                    );
                }
            }
        }

        let spaces = compile_spaces(raw.spaces)?;

        Ok(Config {
            shell,
            modes,
            spaces,
        })
    }

    pub fn bindings(&self, mode: &str) -> Option<&HashMap<Hotkey, Action>> {
        self.modes.get(mode)
    }
}

fn compile_spaces(raw: HashMap<String, RawSpace>) -> Result<Vec<SpaceAssignment>> {
    let mut spaces = Vec::with_capacity(raw.len());
    let mut apps = HashSet::new();
    let mut labels = HashSet::new();

    for (key, space) in raw {
        let index: u32 = key
            .parse()
            .ok()
            .filter(|&i| i >= 1)
            .with_context(|| format!("[spaces] key '{key}' must be a space index >= 1"))?;

        let app = space
            .app
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty());
        let label = space
            .label
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .or_else(|| app.clone())
            .with_context(|| format!("[spaces.{index}] needs `app` or `label`"))?;

        if let Some(app) = &app
            && !apps.insert(app.clone())
        {
            bail!("[spaces.{index}] app '{app}' is already assigned to another space");
        }
        if !labels.insert(label.clone()) {
            bail!("[spaces.{index}] label '{label}' is already used by another space");
        }

        spaces.push(SpaceAssignment {
            index,
            label,
            app,
            follow: space.follow,
        });
    }

    spaces.sort_by_key(|s| s.index);
    Ok(spaces)
}

fn compile_bindings(
    mode: &str,
    raw: HashMap<String, RawAction>,
) -> Result<HashMap<Hotkey, Action>> {
    let mut out = HashMap::with_capacity(raw.len());
    for (chord, action) in raw {
        let hotkey: Hotkey = chord.parse().with_context(|| format!("in mode '{mode}'"))?;
        let action = action
            .compile()
            .with_context(|| format!("binding '{chord}' in mode '{mode}'"))?;
        if out.insert(hotkey, action).is_some() {
            bail!("hotkey '{hotkey}' is bound twice in mode '{mode}' (via '{chord}')");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_action_forms_and_modes() {
        let cfg = Config::parse(
            r#"
            [settings]
            shell = "/bin/zsh"

            [bindings]
            "alt - 1" = "space --focus 1"
            "alt + shift - return" = { shell = "open -a Ghostty" }
            "alt - r" = { mode = "resize" }

            [modes.resize]
            "h" = { yabai = "window --resize left:-20:0" }
            "escape" = { mode = "default" }
            "#,
        )
        .unwrap();

        assert_eq!(cfg.shell, "/bin/zsh");
        let default = cfg.bindings(DEFAULT_MODE).unwrap();
        assert_eq!(
            default[&"alt - 1".parse().unwrap()],
            Action::Yabai(vec!["space".into(), "--focus".into(), "1".into()])
        );
        assert_eq!(
            default[&"alt + shift - return".parse().unwrap()],
            Action::Shell("open -a Ghostty".into())
        );
        let resize = cfg.bindings("resize").unwrap();
        assert_eq!(
            resize[&"escape".parse().unwrap()],
            Action::Mode("default".into())
        );
    }

    #[test]
    fn rejects_unknown_mode_target() {
        let err = Config::parse(
            r#"[bindings]
"alt - r" = { mode = "nope" }"#,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown mode 'nope'"),
            "{err:#}"
        );
    }

    #[test]
    fn rejects_duplicate_hotkey_spelled_differently() {
        let err = Config::parse(
            r#"[bindings]
"alt - return" = "space --focus 1"
"alt - enter" = "space --focus 2""#,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("bound twice"), "{err:#}");
    }

    #[test]
    fn rejects_action_with_two_kinds() {
        let err = Config::parse(
            r#"[bindings]
"alt - a" = { yabai = "x", shell = "y" }"#,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("exactly one"), "{err:#}");
    }

    #[test]
    fn rejects_unknown_top_level_keys() {
        assert!(Config::parse("[bindngs]\n\"alt - a\" = \"x\"").is_err());
    }

    #[test]
    fn parses_spaces_sorted_with_label_defaulting_to_app() {
        let cfg = Config::parse(
            r#"
            [spaces]
            3 = { app = "Ghostty", follow = true }
            1 = { app = "Google Chrome" }
            2 = { app = "Microsoft Teams", label = "teams" }
            4 = { label = "scratch" }
            "#,
        )
        .unwrap();

        assert_eq!(
            cfg.spaces,
            vec![
                SpaceAssignment {
                    index: 1,
                    label: "Google Chrome".into(),
                    app: Some("Google Chrome".into()),
                    follow: false
                },
                SpaceAssignment {
                    index: 2,
                    label: "teams".into(),
                    app: Some("Microsoft Teams".into()),
                    follow: false
                },
                SpaceAssignment {
                    index: 3,
                    label: "Ghostty".into(),
                    app: Some("Ghostty".into()),
                    follow: true
                },
                SpaceAssignment {
                    index: 4,
                    label: "scratch".into(),
                    app: None,
                    follow: false
                },
            ]
        );
    }

    #[test]
    fn rejects_invalid_space_entries() {
        let cases = [
            ("[spaces]\n0 = { app = \"A\" }", "index >= 1"),
            ("[spaces]\nfoo = { app = \"A\" }", "index >= 1"),
            ("[spaces]\n1 = { follow = true }", "needs `app` or `label`"),
            (
                "[spaces]\n1 = { app = \"A\" }\n2 = { app = \"A\" }",
                "already assigned",
            ),
            (
                "[spaces]\n1 = { app = \"A\" }\n2 = { label = \"A\" }",
                "already used",
            ),
            ("[spaces]\n1 = { app = \"A\", space = 2 }", "unknown field"),
        ];
        for (text, expected) in cases {
            let err = Config::parse(text).unwrap_err();
            assert!(format!("{err:#}").contains(expected), "{text}: {err:#}");
        }
    }
}
