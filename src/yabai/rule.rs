//! `yabai -m rule` wrappers.
//!
//! Rules are identified by label. `rule --add` with an existing label replaces
//! that rule in place (verified against yabai 7.1), so re-adding is idempotent.

use anyhow::Result;
use serde::Deserialize;

use super::Client;

/// One entry of `yabai -m rule --list`. Only the fields we read.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Rule {
    pub index: u32,
    pub label: String,
    pub app: String,
    pub title: String,
    pub space: u64,
    pub follow_space: bool,
    pub manage: Option<bool>,
}

/// A rule that sends windows of an app to a space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSpaceRule {
    pub label: String,
    /// Regex matched against the application name.
    pub app_regex: String,
    /// Space selector: index or label.
    pub space: String,
    /// `space=^<sel>` makes yabai focus the space when the rule applies.
    pub follow: bool,
}

impl AppSpaceRule {
    /// Arguments after `rule --add` / `rule --apply`.
    pub fn args(&self) -> Vec<String> {
        let space = if self.follow {
            format!("space=^{}", self.space)
        } else {
            format!("space={}", self.space)
        };
        vec![
            format!("label={}", self.label),
            format!("app={}", self.app_regex),
            space,
        ]
    }
}

/// Anchors exact app names as a yabai regex: `["Ghostty"]` → `^Ghostty$`,
/// `["A", "B"]` → `^(A|B)$`, so `Ghostty` won't also match `Ghostty Helper`.
/// Regex metacharacters are escaped.
pub fn exact_app_regex<S: AsRef<str>>(apps: &[S]) -> String {
    let escaped: Vec<String> = apps
        .iter()
        .map(|app| {
            let app = app.as_ref();
            let mut out = String::with_capacity(app.len());
            for c in app.chars() {
                if r"\.^$|?*+()[]{}".contains(c) {
                    out.push('\\');
                }
                out.push(c);
            }
            out
        })
        .collect();
    match escaped.as_slice() {
        [one] => format!("^{one}$"),
        many => format!("^({})$", many.join("|")),
    }
}

impl Client {
    pub fn rules(&self) -> Result<Vec<Rule>> {
        self.query(&["rule", "--list"])
    }

    /// Adds or replaces the rule, then applies it to windows that already
    /// exist so they move immediately.
    pub fn add_rule(&self, rule: &AppSpaceRule) -> Result<()> {
        let mut args = vec!["rule".to_string(), "--add".to_string()];
        args.extend(rule.args());
        self.send(&args)?;
        self.send(&["rule", "--apply", &rule.label])?;
        Ok(())
    }

    pub fn remove_rule(&self, label: &str) -> Result<()> {
        self.send(&["rule", "--remove", label])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_regex_anchors_and_escapes() {
        assert_eq!(exact_app_regex(&["Ghostty"]), "^Ghostty$");
        assert_eq!(exact_app_regex(&["Logi Options+"]), r"^Logi Options\+$");
        assert_eq!(exact_app_regex(&["a.b (c)"]), r"^a\.b \(c\)$");
        assert_eq!(
            exact_app_regex(&["Microsoft Teams", "Logi Options+"]),
            r"^(Microsoft Teams|Logi Options\+)$"
        );
    }

    #[test]
    fn follow_prefixes_space_selector_with_caret() {
        let rule = AppSpaceRule {
            label: "l".into(),
            app_regex: "^A$".into(),
            space: "3".into(),
            follow: true,
        };
        assert_eq!(rule.args(), vec!["label=l", "app=^A$", "space=^3"]);
        let no_follow = AppSpaceRule {
            follow: false,
            ..rule
        };
        assert_eq!(no_follow.args()[2], "space=3");
    }
}
