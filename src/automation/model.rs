use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::session::View;

fn default_keep_last() -> u32 {
    5
}
fn default_max_runtime_secs() -> u64 {
    1800
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Trigger {
    /// 5-field cron expression evaluated in the user's local timezone.
    Cron { expr: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMode {
    #[default]
    Fresh,
    Persistent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retention {
    #[serde(default = "default_keep_last")]
    pub keep_last: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            keep_last: default_keep_last(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub project_path: String,
    #[serde(default)]
    pub group_path: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub extra_args: String,
    #[serde(default)]
    pub view: View,
    #[serde(default)]
    pub worktree_branch: Option<String>,
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default = "crate::automation::model::default_auto_approve")]
    pub auto_approve: bool,
    #[serde(default = "default_max_runtime_secs")]
    pub max_runtime_secs: u64,
    #[serde(default)]
    pub initial_prompt: String,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub agent_model: Option<String>,
}

pub fn default_auto_approve() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunOutcome {
    /// Dispatched and in flight: the run has launched but not yet finished. The
    /// completion loop transitions this to `Completed`, and `enforce_max_runtime`
    /// keys its "still running" check off this variant.
    Running,
    Completed,
    TimedOut,
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub at: DateTime<Utc>,
    pub session_id: String,
    pub outcome: RunOutcome,
    /// When the initial prompt was injected into the run's session. `None` until
    /// injection succeeds. The completion loop gates finalization on this so a
    /// startup `Running -> Idle` flicker before injection cannot archive the run
    /// prematurely (ADR-0003).
    #[serde(default)]
    pub injected_at: Option<DateTime<Utc>>,
}

/// Whether a `Running -> Idle` transition at `transition_at` should finalize the
/// run: only once the initial prompt has been injected, and only for a
/// transition at or after that injection (ADR-0003, "first Running->Idle AFTER
/// the initial prompt is injected"). A `None` `injected_at` means the prompt has
/// not been sent yet, so the transition is a startup flicker to ignore.
pub fn should_finalize(injected_at: Option<DateTime<Utc>>, transition_at: DateTime<Utc>) -> bool {
    match injected_at {
        Some(t) => transition_at >= t,
        None => false,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutomationState {
    #[serde(default)]
    pub last_run: Option<RunRecord>,
    #[serde(default)]
    pub next_fire: Option<DateTime<Utc>>,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub pending_fire: bool,
    #[serde(default)]
    pub persistent_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    #[serde(default = "crate::automation::model::default_enabled")]
    pub enabled: bool,
    pub trigger: Trigger,
    pub spec: LaunchSpec,
    #[serde(default)]
    pub session_mode: SessionMode,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub state: AutomationState,
}

pub fn default_enabled() -> bool {
    true
}

impl Automation {
    pub fn new(name: &str, spec: LaunchSpec, trigger: Trigger) -> Self {
        Automation {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            enabled: true,
            trigger,
            spec,
            session_mode: SessionMode::Fresh,
            retention: Retention::default(),
            state: AutomationState::default(),
        }
    }

    /// Short, stable display id (first 8 chars of the uuid).
    pub fn short_id(&self) -> &str {
        &self.id[..8]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_round_trips_through_json() {
        let spec = LaunchSpec {
            project_path: "/home/me/proj".into(),
            group_path: String::new(),
            tool: Some("claude".into()),
            command: None,
            extra_args: String::new(),
            view: crate::session::View::Terminal,
            worktree_branch: None,
            sandbox: true,
            auto_approve: true,
            max_runtime_secs: 1800,
            initial_prompt: "summarize my slack".into(),
            agent_name: None,
            agent_model: None,
        };
        let a = Automation::new(
            "slack digest",
            spec,
            Trigger::Cron {
                expr: "*/30 * * * *".into(),
            },
        );
        let json = serde_json::to_string(&a).unwrap();
        let back: Automation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "slack digest");
        assert_eq!(back.session_mode, SessionMode::Fresh);
        assert_eq!(back.retention.keep_last, 5);
        assert_eq!(back.short_id().len(), 8);
        assert!(matches!(back.trigger, Trigger::Cron { .. }));
    }

    #[test]
    fn should_finalize_ignores_uninjected_run() {
        // No injection yet: a startup flicker must not finalize.
        assert!(!should_finalize(None, Utc::now()));
    }

    #[test]
    fn should_finalize_requires_transition_at_or_after_injection() {
        let injected = Utc::now();
        // Transition before injection (clock skew / earlier flicker) is ignored.
        assert!(!should_finalize(
            Some(injected),
            injected - chrono::Duration::milliseconds(1)
        ));
        // Transition at or after injection finalizes.
        assert!(should_finalize(Some(injected), injected));
        assert!(should_finalize(
            Some(injected),
            injected + chrono::Duration::seconds(1)
        ));
    }
}
