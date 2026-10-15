//! Applies `[spaces]` assignments to yabai: creates missing spaces, labels
//! them, and installs one app→space rule per assignment.
//!
//! The work is split into a pure [`plan`] (config + current yabai spaces →
//! list of [`Step`]s) and an [`apply`] that executes steps, so the decision
//! logic is testable without a yabai socket.

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::config::SpaceAssignment;
use crate::yabai::Client;
use crate::yabai::model::Space;
use crate::yabai::rule::{AppSpaceRule, exact_app_regex};

pub const RULE_LABEL_PREFIX: &str = "yabai_controls_space_";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    CreateSpace,
    Label { index: u32, label: String },
    AddRule(AppSpaceRule),
    RemoveRule(String),
}

fn rule_label(index: u32) -> String {
    format!("{RULE_LABEL_PREFIX}{index}")
}

fn is_our_rule(label: &str) -> bool {
    label.starts_with(RULE_LABEL_PREFIX)
}

/// The rule a assignment needs, if it has an app.
fn rule_for(assignment: &SpaceAssignment) -> Option<AppSpaceRule> {
    let app = assignment.app.as_deref()?;
    Some(AppSpaceRule {
        label: rule_label(assignment.index),
        app_regex: exact_app_regex(app),
        // Target by index, not label: the label may not be applied yet when
        // yabai evaluates the rule, and an index is unambiguous.
        space: assignment.index.to_string(),
        follow: assignment.follow,
    })
}

/// How many spaces must be created so every assignment's index exists.
pub fn missing_spaces(existing_count: usize, assignments: &[SpaceAssignment]) -> u32 {
    let needed = assignments.iter().map(|s| s.index).max().unwrap_or(0);
    needed.saturating_sub(existing_count as u32)
}

/// Steps plus the assignments that could not be planned.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Indices in `new` with no matching space in yabai. Their label and rule
    /// are skipped; nothing else about them is applied.
    pub skipped: Vec<u32>,
}

/// Computes the steps that bring yabai in line with `new`, given the spaces
/// and the rule labels yabai currently reports. Never emits `CreateSpace`;
/// see [`missing_spaces`].
///
/// Order: remove stale rules, label, add rules. A rule is stale when it has
/// our label prefix (`existing_rules` is filtered here) and no present
/// assignment with an app wants it; this catches rules left behind by a
/// previous run that did not tear down, not just entries dropped from the
/// config. Labels are only re-sent when they differ. Rules are always re-sent
/// because `--add` replaces by label and `--apply` moves windows that opened
/// in the meantime. Assignments whose space does not exist are reported in
/// `skipped`.
pub fn plan(existing: &[Space], existing_rules: &[String], new: &[SpaceAssignment]) -> Plan {
    let mut plan = Plan::default();
    let space_exists = |index: u32| existing.iter().any(|s| s.index == index);

    let (present, missing): (Vec<&SpaceAssignment>, Vec<&SpaceAssignment>) =
        new.iter().partition(|a| space_exists(a.index));
    plan.skipped = missing.iter().map(|a| a.index).collect();

    let wanted: Vec<String> = present
        .iter()
        .filter(|a| a.app.is_some())
        .map(|a| rule_label(a.index))
        .collect();
    for label in existing_rules.iter().filter(|l| is_our_rule(l)) {
        if !wanted.contains(label) {
            plan.steps.push(Step::RemoveRule(label.clone()));
        }
    }

    for assignment in &present {
        let current_label = existing
            .iter()
            .find(|s| s.index == assignment.index)
            .map(|s| s.label.as_str());
        if current_label != Some(assignment.label.as_str()) {
            plan.steps.push(Step::Label {
                index: assignment.index,
                label: assignment.label.clone(),
            });
        }
    }

    plan.steps.extend(
        present
            .iter()
            .filter_map(|a| rule_for(a))
            .map(Step::AddRule),
    );

    plan
}

/// Steps that remove every rule of ours among `existing_rules`; labels are
/// left in place.
pub fn teardown(existing_rules: &[String]) -> Vec<Step> {
    existing_rules
        .iter()
        .filter(|l| is_our_rule(l))
        .map(|l| Step::RemoveRule(l.clone()))
        .collect()
}

/// Labels of every rule yabai currently has.
pub fn rule_labels(client: &Client) -> Result<Vec<String>> {
    Ok(client
        .rules()
        .context("querying rules")?
        .into_iter()
        .map(|r| r.label)
        .collect())
}

/// Removes every rule of ours from yabai. Failures are logged.
pub fn remove_all_rules(client: &Client) {
    match rule_labels(client) {
        Ok(labels) => {
            apply(client, &teardown(&labels));
        }
        Err(err) => warn!(%err, "could not list rules for teardown"),
    }
}

/// Executes steps in order. Every failure is logged and the remaining steps
/// still run. Returns the number of failed steps.
pub fn apply(client: &Client, steps: &[Step]) -> usize {
    let mut failures = 0;
    for step in steps {
        debug!(?step, "applying space step");
        let result = match step {
            Step::CreateSpace => client
                .create_space()
                .context("creating space (is the scripting addition loaded?)"),
            Step::Label { index, label } => client
                .label_space(*index, label)
                .with_context(|| format!("labelling space {index} '{label}'")),
            Step::AddRule(rule) => client
                .add_rule(rule)
                .with_context(|| format!("adding rule {}", rule.label)),
            Step::RemoveRule(label) => client
                .remove_rule(label)
                .with_context(|| format!("removing rule {label}")),
        };
        if let Err(err) = result {
            failures += 1;
            warn!(err = format!("{err:#}"), "space step failed");
        }
    }
    failures
}

/// Brings yabai in line with `new`: creates missing spaces where it can,
/// then labels and installs rules for every assignment whose space exists,
/// and removes any of our rules that `new` no longer wants.
///
/// Only an unreachable yabai is an error. A space that cannot be created
/// (no scripting addition) or a step that yabai rejects is logged and the
/// rest still applies, so the daemon always starts.
pub fn sync(client: &Client, new: &[SpaceAssignment]) -> Result<()> {
    let mut existing = client.spaces().context("querying spaces")?;

    let missing = missing_spaces(existing.len(), new);
    if missing > 0 {
        let mut created = 0;
        for _ in 0..missing {
            match client.create_space() {
                Ok(()) => created += 1,
                Err(err) => {
                    warn!(
                        %err,
                        wanted = missing,
                        created,
                        "could not create spaces; assignments for missing spaces will be skipped \
                         (load the scripting addition with `sudo yabai --load-sa`)"
                    );
                    break;
                }
            }
        }
        if created > 0 {
            info!(created, "spaces created");
            existing = client.spaces().context("re-querying spaces")?;
        }
    }

    let plan = plan(&existing, &rule_labels(client)?, new);
    for index in &plan.skipped {
        if let Some(a) = new.iter().find(|a| a.index == *index) {
            warn!(
                index,
                label = %a.label,
                app = a.app.as_deref().unwrap_or("-"),
                "space does not exist; skipping its label and rule"
            );
        }
    }

    if plan.steps.is_empty() {
        debug!("spaces already in sync");
        return Ok(());
    }
    let failures = apply(client, &plan.steps);
    info!(
        steps = plan.steps.len(),
        failures,
        skipped = plan.skipped.len(),
        "space assignments applied"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assign(index: u32, app: Option<&str>, label: &str, follow: bool) -> SpaceAssignment {
        SpaceAssignment {
            index,
            label: label.into(),
            app: app.map(Into::into),
            follow,
        }
    }

    fn space(index: u32, label: &str) -> Space {
        Space {
            index,
            label: label.into(),
            ..Default::default()
        }
    }

    #[test]
    fn missing_spaces_counts_up_to_the_highest_index() {
        assert_eq!(missing_spaces(2, &[assign(1, None, "a", false)]), 0);
        assert_eq!(missing_spaces(2, &[assign(5, None, "a", false)]), 3);
        assert_eq!(missing_spaces(2, &[]), 0);
    }

    #[test]
    fn first_run_labels_then_adds_rules() {
        let existing = vec![space(1, ""), space(2, ""), space(3, "")];
        let new = vec![
            assign(1, Some("Google Chrome"), "Google Chrome", false),
            assign(3, Some("Ghostty"), "Ghostty", true),
        ];

        let plan = plan(&existing, &[], &new);

        assert!(plan.skipped.is_empty());
        assert_eq!(
            plan.steps,
            vec![
                Step::Label {
                    index: 1,
                    label: "Google Chrome".into()
                },
                Step::Label {
                    index: 3,
                    label: "Ghostty".into()
                },
                Step::AddRule(AppSpaceRule {
                    label: "yabai_controls_space_1".into(),
                    app_regex: "^Google Chrome$".into(),
                    space: "1".into(),
                    follow: false,
                }),
                Step::AddRule(AppSpaceRule {
                    label: "yabai_controls_space_3".into(),
                    app_regex: "^Ghostty$".into(),
                    space: "3".into(),
                    follow: true,
                }),
            ]
        );
    }

    #[test]
    fn assignment_for_nonexistent_space_is_skipped_not_planned() {
        let existing = vec![space(1, ""), space(2, "")];
        let new = vec![
            assign(1, Some("A"), "A", false),
            assign(5, Some("Microsoft Teams"), "teams", false),
        ];

        let plan = plan(&existing, &[], &new);

        assert_eq!(plan.skipped, vec![5]);
        let touches_space_5 = plan.steps.iter().any(|s| match s {
            Step::Label { index, .. } => *index == 5,
            Step::AddRule(rule) => rule.space == "5",
            _ => false,
        });
        assert!(!touches_space_5, "{:?}", plan.steps);
        assert_eq!(
            plan.steps
                .iter()
                .filter(|s| matches!(s, Step::AddRule(_)))
                .count(),
            1
        );
    }

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn rule_previously_applied_to_a_now_missing_space_is_removed() {
        // Space 3 was destroyed between runs; its old rule must go.
        let existing = vec![space(1, "A"), space(2, "")];
        let new = vec![
            assign(1, Some("A"), "A", false),
            assign(3, Some("C"), "C", false),
        ];
        let rules = labels(&["yabai_controls_space_1", "yabai_controls_space_3"]);

        let plan = plan(&existing, &rules, &new);

        assert_eq!(plan.skipped, vec![3]);
        assert_eq!(
            plan.steps[0],
            Step::RemoveRule("yabai_controls_space_3".into())
        );
    }

    #[test]
    fn unchanged_label_is_not_resent_but_rule_is() {
        let existing = vec![space(1, "Ghostty")];
        let new = vec![assign(1, Some("Ghostty"), "Ghostty", false)];

        let plan = plan(&existing, &labels(&["yabai_controls_space_1"]), &new);

        assert!(!plan.steps.iter().any(|s| matches!(s, Step::Label { .. })));
        assert_eq!(
            plan.steps
                .iter()
                .filter(|s| matches!(s, Step::AddRule(_)))
                .count(),
            1
        );
    }

    #[test]
    fn label_only_assignment_adds_no_rule() {
        let plan = plan(&[space(1, "")], &[], &[assign(1, None, "scratch", false)]);
        assert_eq!(
            plan.steps,
            vec![Step::Label {
                index: 1,
                label: "scratch".into()
            }]
        );
    }

    #[test]
    fn stale_rules_are_removed_and_foreign_rules_are_left_alone() {
        // Rules in yabai: 1 keeps its app (rule replaced), 2 loses its app,
        // 3 is dropped from the config, 4 was left by a run that never tore
        // down, and the user's own rule must never be touched.
        let rules = labels(&[
            "yabai_controls_space_1",
            "yabai_controls_space_2",
            "yabai_controls_space_3",
            "yabai_controls_space_4",
            "my_own_rule",
        ]);
        let new = vec![
            assign(1, Some("A2"), "A", false),
            assign(2, None, "B", false),
        ];
        let existing = vec![space(1, "A"), space(2, "B"), space(3, "C")];

        let plan = plan(&existing, &rules, &new);

        let removed: Vec<_> = plan
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::RemoveRule(l) => Some(l.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            removed,
            vec![
                "yabai_controls_space_2",
                "yabai_controls_space_3",
                "yabai_controls_space_4"
            ]
        );
        assert!(
            matches!(&plan.steps[0], Step::RemoveRule(_)),
            "removals come first"
        );
    }

    #[test]
    fn teardown_removes_only_our_rules() {
        let steps = teardown(&labels(&[
            "my_own_rule",
            "yabai_controls_space_1",
            "yabai_controls_space_7",
        ]));
        assert_eq!(
            steps,
            vec![
                Step::RemoveRule("yabai_controls_space_1".into()),
                Step::RemoveRule("yabai_controls_space_7".into()),
            ]
        );
    }
}
