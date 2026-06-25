//! Automations view - browse and manage scheduled automations.
//!
//! A full-screen takeover (like Settings/Diff/Serve) listing the automations
//! from `automations.json`, showing each one's schedule, `next_fire`, and
//! `last_run`, and supporting enable/disable, delete, and run-now. Add/edit
//! reuse the new-session wizard (see `home::input`).

mod input;
mod render;

use anyhow::Result;

use crate::automation::model::{Automation, RunOutcome};
use crate::automation::store::AutomationStore;

/// What the host (`HomeView`) should do after the view handles a key.
pub enum AutomationsAction {
    /// Stay in the view.
    Continue,
    /// Close the view and return to the session list.
    Close,
    /// Open the new-session wizard to collect a launch spec for a new
    /// automation. The host owns the wizard, so it routes the result back
    /// through automation creation rather than `create_session`.
    StartAdd,
    /// Open the schedule dialog to edit the given automation.
    StartEdit(Box<Automation>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    List,
    ConfirmDelete,
}

pub struct AutomationsView {
    profile: String,
    automations: Vec<Automation>,
    selected: usize,
    mode: Mode,
    /// Sticky error banner (store failures, dispatch failures). Cleared on the
    /// next navigation/action keystroke.
    error: Option<String>,
    /// Transient status line (e.g. "Launching run..."). Same lifetime as `error`.
    status: Option<String>,
}

impl AutomationsView {
    pub fn new(profile: &str) -> Result<Self> {
        let store = AutomationStore::new(profile)?;
        let automations = store.load()?;
        Ok(Self {
            profile: profile.to_string(),
            automations,
            selected: 0,
            mode: Mode::List,
            error: None,
            status: None,
        })
    }

    fn store(&self) -> Result<AutomationStore> {
        AutomationStore::new(&self.profile)
    }

    /// Re-read the store after a mutation and clamp the cursor.
    fn reload(&mut self) {
        match self.store().and_then(|s| s.load()) {
            Ok(list) => {
                self.automations = list;
                self.clamp_selection();
            }
            Err(e) => self.error = Some(format!("Failed to reload automations: {e}")),
        }
    }

    fn clamp_selection(&mut self) {
        if self.automations.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.automations.len() {
            self.selected = self.automations.len() - 1;
        }
    }

    fn selected_automation(&self) -> Option<&Automation> {
        self.automations.get(self.selected)
    }

    fn select_next(&mut self) {
        if !self.automations.is_empty() {
            self.selected = (self.selected + 1).min(self.automations.len() - 1);
        }
    }

    fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Toggle enabled on the selected automation, mirroring the CLI's
    /// `enable`/`disable`: re-enabling clears `next_fire` so the scheduler
    /// recomputes it on the next tick.
    fn toggle_enabled(&mut self) {
        let Some(target) = self.selected_automation() else {
            return;
        };
        let id = target.id.clone();
        let now_enabled = !target.enabled;
        let store = match self.store() {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(format!("Failed to open store: {e}"));
                return;
            }
        };
        let result = store.update(|list| {
            if let Some(a) = list.iter_mut().find(|a| a.id == id) {
                a.enabled = now_enabled;
                if now_enabled {
                    a.state.next_fire = None;
                }
            }
            Ok(())
        });
        if let Err(e) = result {
            self.error = Some(format!("Failed to toggle automation: {e}"));
            return;
        }
        self.reload();
    }

    fn delete_selected(&mut self) {
        let Some(target) = self.selected_automation() else {
            return;
        };
        let id = target.id.clone();
        let store = match self.store() {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(format!("Failed to open store: {e}"));
                return;
            }
        };
        let result = store.update(|list| {
            list.retain(|a| a.id != id);
            Ok(())
        });
        if let Err(e) = result {
            self.error = Some(format!("Failed to delete automation: {e}"));
            return;
        }
        self.reload();
    }

    /// Fire the selected automation now, mirroring the CLI's `run-now`. The
    /// dispatch is async and may spawn a tmux session (~hundreds of ms), so it
    /// runs on a detached thread driving the ambient tokio runtime rather than
    /// blocking the UI. The new `(auto)` session shows up in the session list;
    /// the daemon's completion loop records `last_run`.
    fn run_now(&mut self) {
        let Some(automation) = self.selected_automation().cloned() else {
            return;
        };
        let profile = self.profile.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let name = automation.name.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle.block_on(crate::automation::dispatch::launch_run(
                        &automation,
                        &profile,
                    )) {
                        tracing::error!(target: "tui.automations", "run-now failed: {e}");
                    }
                });
                self.status = Some(format!("Launching run for \"{name}\"..."));
            }
            Err(_) => {
                self.error = Some("Cannot run now: no async runtime available.".into());
            }
        }
    }
}

/// Glyph for a run outcome (or `None` = never run), used in the list rows.
fn outcome_glyph(outcome: Option<&RunOutcome>) -> &'static str {
    match outcome {
        None => "·",
        Some(RunOutcome::Running) => "▶",
        Some(RunOutcome::Completed) => "✓",
        Some(RunOutcome::TimedOut) => "⧗",
        Some(RunOutcome::Failed { .. }) => "✗",
    }
}

/// Compact relative time until `target` from `now`, e.g. "in 4h", "in 3d",
/// "now", "past". Negative deltas (a stale `next_fire` the daemon hasn't
/// advanced yet) render as "past".
fn humanize_until(
    now: chrono::DateTime<chrono::Utc>,
    target: chrono::DateTime<chrono::Utc>,
) -> String {
    let secs = (target - now).num_seconds();
    if secs < 0 {
        return "past".to_string();
    }
    if secs < 60 {
        return "now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("in {mins}m");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("in {hours}h");
    }
    let days = hours / 24;
    format!("in {days}d")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::model::{Automation, LaunchSpec, Trigger};
    use chrono::{Duration, Utc};

    fn spec() -> LaunchSpec {
        LaunchSpec {
            project_path: "/tmp/p".into(),
            group_path: String::new(),
            tool: Some("claude".into()),
            command: None,
            extra_args: String::new(),
            view: crate::session::View::Terminal,
            worktree_branch: None,
            sandbox: false,
            auto_approve: true,
            max_runtime_secs: 1800,
            initial_prompt: "hi".into(),
            agent_name: None,
            agent_model: None,
        }
    }

    fn view_with(n: usize) -> AutomationsView {
        let automations = (0..n)
            .map(|i| {
                Automation::new(
                    &format!("auto {i}"),
                    spec(),
                    Trigger::Cron {
                        expr: "* * * * *".into(),
                    },
                )
            })
            .collect();
        AutomationsView {
            profile: "default".into(),
            automations,
            selected: 0,
            mode: Mode::List,
            error: None,
            status: None,
        }
    }

    #[test]
    fn navigation_clamps_at_both_ends() {
        let mut v = view_with(3);
        v.select_prev(); // already at 0, stays
        assert_eq!(v.selected, 0);
        v.select_next();
        v.select_next();
        v.select_next(); // past the end, clamps to 2
        assert_eq!(v.selected, 2);
        v.select_prev();
        assert_eq!(v.selected, 1);
    }

    #[test]
    fn navigation_on_empty_list_is_safe() {
        let mut v = view_with(0);
        v.select_next();
        v.select_prev();
        assert_eq!(v.selected, 0);
        assert!(v.selected_automation().is_none());
    }

    #[test]
    fn clamp_selection_after_delete_shrinks_cursor() {
        let mut v = view_with(3);
        v.selected = 2;
        v.automations.pop();
        v.clamp_selection();
        assert_eq!(v.selected, 1);
    }

    #[test]
    fn outcome_glyphs_distinguish_states() {
        assert_eq!(outcome_glyph(None), "·");
        assert_eq!(outcome_glyph(Some(&RunOutcome::Completed)), "✓");
        assert_eq!(
            outcome_glyph(Some(&RunOutcome::Failed { reason: "x".into() })),
            "✗"
        );
    }

    #[test]
    fn humanize_until_buckets() {
        let now = Utc::now();
        assert_eq!(humanize_until(now, now - Duration::seconds(10)), "past");
        assert_eq!(humanize_until(now, now + Duration::seconds(5)), "now");
        assert_eq!(humanize_until(now, now + Duration::minutes(5)), "in 5m");
        assert_eq!(humanize_until(now, now + Duration::hours(4)), "in 4h");
        assert_eq!(humanize_until(now, now + Duration::days(3)), "in 3d");
    }
}
