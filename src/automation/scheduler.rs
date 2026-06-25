use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::automation::cron;
use crate::automation::model::Automation;
use crate::automation::store::AutomationStore;
#[cfg(feature = "serve")]
use crate::session::Instance;

/// Session ids to prune, given newest-first order and a keep-last count.
pub fn runs_to_prune(sessions_newest_first: &[String], keep_last: u32) -> Vec<String> {
    sessions_newest_first
        .iter()
        .skip(keep_last as usize)
        .cloned()
        .collect()
}

pub fn due_automations(list: &[Automation], now: DateTime<Utc>) -> Vec<usize> {
    list.iter()
        .enumerate()
        .filter(|(_, a)| a.enabled)
        .filter(|(_, a)| match a.state.next_fire {
            None => true,
            Some(t) => t <= now,
        })
        .map(|(i, _)| i)
        .collect()
}

/// True iff dispatching `a` now would collide with its own still-busy
/// persistent session: the automation is `Persistent` and its recorded
/// `persistent_session_id` is among the currently-running session ids. Fresh
/// mode, a missing persistent id, and an id that is not running all return
/// false.
pub fn persistent_collision(
    a: &Automation,
    running_ids: &std::collections::HashSet<String>,
) -> bool {
    if a.session_mode != crate::automation::model::SessionMode::Persistent {
        return false;
    }
    match &a.state.persistent_session_id {
        Some(sid) => running_ids.contains(sid),
        None => false,
    }
}

pub fn ensure_next_fire(a: &mut Automation, now: DateTime<Utc>) {
    let crate::automation::model::Trigger::Cron { expr } = &a.trigger;
    match cron::next_fire_after(expr, now) {
        Ok(next) => a.state.next_fire = Some(next),
        Err(e) => {
            tracing::warn!(target: "automation", id = %a.short_id(), error = %e, "bad cron; disabling");
            a.enabled = false;
        }
    }
}

#[cfg(feature = "serve")]
pub async fn automation_poll_loop(state: Arc<crate::server::AppState>) {
    let profile = state.profile.clone();
    let store = match AutomationStore::new(&profile) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(target: "automation", error = %e, "cannot open automation store; loop exiting");
            return;
        }
    };
    let cfg = crate::session::config::load_config()
        .ok()
        .flatten()
        .unwrap_or_default()
        .automation;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(
        cfg.scheduler_tick_secs.max(5),
    ));

    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = state.shutdown.cancelled() => return,
        }
        let now = Utc::now();

        // Stop any automation run that has outrun its max_runtime budget.
        enforce_max_runtime(&state, &store, now).await;

        // Count automation-launched sessions currently running (concurrency cap).
        let running = count_running_auto_sessions(&state).await;
        let mut budget = cfg.max_concurrent_runs.saturating_sub(running);
        if budget == 0 {
            continue;
        }

        let snapshot = match store.load() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let due = due_automations(&snapshot, now);

        // Session ids currently running/starting, so a persistent automation
        // whose session is still busy can defer instead of double-firing.
        let running_ids = running_session_ids(&state).await;

        for idx in due {
            if budget == 0 {
                break;
            }
            let automation = snapshot[idx].clone();

            // First-time initialization: just set next_fire, do not run.
            if automation.state.next_fire.is_none() {
                store
                    .update(|list| {
                        if let Some(a) = list.iter_mut().find(|a| a.id == automation.id) {
                            ensure_next_fire(a, now);
                        }
                        Ok(())
                    })
                    .ok();
                continue;
            }

            // Busy collision: the persistent session is still running. Defer
            // (coalesce) this fire instead of interrupting; do not spend
            // budget or launch a run (ADR-0003).
            if persistent_collision(&automation, &running_ids) {
                store
                    .update(|list| {
                        if let Some(a) = list.iter_mut().find(|a| a.id == automation.id) {
                            a.state.pending_fire = true;
                            ensure_next_fire(a, now);
                        }
                        Ok(())
                    })
                    .ok();
                continue;
            }

            // Dispatch a run for this automation (Task 8 handles completion).
            match crate::automation::dispatch::launch_run(&automation, &profile).await {
                Ok(dispatched) => {
                    let session_id = dispatched.session_id;
                    budget -= 1;
                    store
                        .update(|list| {
                            if let Some(a) = list.iter_mut().find(|a| a.id == automation.id) {
                                ensure_next_fire(a, now);
                                a.state.last_run = Some(crate::automation::model::RunRecord {
                                    at: now,
                                    session_id: session_id.clone(),
                                    outcome: crate::automation::model::RunOutcome::Running,
                                    injected_at: dispatched.injected_at,
                                });
                                // Record the persistent session on first fire so
                                // subsequent runs reuse it.
                                if a.session_mode
                                    == crate::automation::model::SessionMode::Persistent
                                    && a.state.persistent_session_id.is_none()
                                {
                                    a.state.persistent_session_id = Some(session_id.clone());
                                }
                            }
                            Ok(())
                        })
                        .ok();
                }
                Err(e) => {
                    tracing::warn!(target: "automation", id = %automation.short_id(), error = %e, "run dispatch failed");
                    record_failure(&store, &automation.id, &profile, e.to_string()).await;
                }
            }
        }
    }
}

/// Count of automation-launched sessions currently `Running` or `Starting`,
/// used as the concurrency cap. This reads `state.instances`, which is updated
/// by the server's status reconciler and so lags a just-dispatched run by up to
/// one poll interval. The cap is therefore a soft cap: across consecutive ticks
/// the configured `max_concurrent_runs` can be briefly exceeded before the new
/// session shows up here. This is accepted in v1; dispatch and this count read
/// different sources and reconciling them exactly is out of scope.
#[cfg(feature = "serve")]
async fn count_running_auto_sessions(state: &Arc<crate::server::AppState>) -> u32 {
    let instances = state.instances.read().await;
    instances
        .iter()
        .filter(|i| i.automation_id.is_some())
        .filter(|i| {
            matches!(
                i.status,
                crate::session::Status::Running | crate::session::Status::Starting
            )
        })
        .count() as u32
}

/// Ids of sessions currently `Running` or `Starting`. Used to detect a
/// persistent automation whose session is still busy at fire time.
#[cfg(feature = "serve")]
async fn running_session_ids(
    state: &Arc<crate::server::AppState>,
) -> std::collections::HashSet<String> {
    let instances = state.instances.read().await;
    instances
        .iter()
        .filter(|i| {
            matches!(
                i.status,
                crate::session::Status::Running | crate::session::Status::Starting
            )
        })
        .map(|i| i.id.clone())
        .collect()
}

#[cfg(feature = "serve")]
async fn record_failure(store: &AutomationStore, id: &str, _profile: &str, reason: String) {
    let limit = crate::session::config::load_config()
        .ok()
        .flatten()
        .unwrap_or_default()
        .automation
        .consecutive_failure_limit;
    let now = Utc::now();
    store
        .update(|list| {
            if let Some(a) = list.iter_mut().find(|a| a.id == id) {
                a.state.consecutive_failures += 1;
                a.state.last_run = Some(crate::automation::model::RunRecord {
                    at: now,
                    session_id: String::new(),
                    outcome: crate::automation::model::RunOutcome::Failed { reason },
                    injected_at: None,
                });
                if a.state.consecutive_failures >= limit {
                    a.enabled = false;
                    tracing::error!(target: "automation", id = %a.short_id(), "auto-disabled after repeated failures");
                }
                // Advance to the next scheduled fire even on failure, so a
                // failing automation retries on its cron cadence instead of
                // re-firing every poll tick (which would exhaust the
                // consecutive-failure budget in seconds).
                ensure_next_fire(a, now);
            }
            Ok(())
        })
        .ok();
}

/// Stop automation runs whose elapsed wall-clock time exceeds the
/// automation's `max_runtime_secs`, recording `RunOutcome::TimedOut`. Run
/// start is tracked via the automation's `last_run.at`.
#[cfg(feature = "serve")]
async fn enforce_max_runtime(
    state: &Arc<crate::server::AppState>,
    store: &AutomationStore,
    now: DateTime<Utc>,
) {
    let snapshot = match store.load() {
        Ok(s) => s,
        Err(_) => return,
    };

    // Collect (automation_id, session_id) for runs that have timed out and are
    // still in flight. A run is only a candidate while its last_run outcome is
    // still Running; anything already finalized/failed/timed-out is skipped.
    let mut timed_out: Vec<(String, String)> = Vec::new();
    for a in &snapshot {
        // Persistent sessions are intentionally long-lived across runs, so the
        // max_runtime sweep must never reap them. Note: a wedged INITIAL
        // persistent run (before persistent_session_id is set) is also not
        // reaped in v1; that gap is a tracked follow-up.
        if a.session_mode == crate::automation::model::SessionMode::Persistent {
            continue;
        }
        let Some(run) = a.state.last_run.as_ref() else {
            continue;
        };
        if !matches!(run.outcome, crate::automation::model::RunOutcome::Running) {
            continue;
        }
        let elapsed = (now - run.at).num_seconds();
        if elapsed < a.spec.max_runtime_secs as i64 {
            continue;
        }
        timed_out.push((a.id.clone(), run.session_id.clone()));
    }
    if timed_out.is_empty() {
        return;
    }

    for (auto_id, session_id) in timed_out {
        // Stop the live session if it is still around and running.
        let inst = {
            let instances = state.instances.read().await;
            instances
                .iter()
                .find(|i| i.id == session_id)
                .filter(|i| {
                    matches!(
                        i.status,
                        crate::session::Status::Running | crate::session::Status::Starting
                    )
                })
                .cloned()
        };
        if let Some(inst) = inst {
            if let Err(e) = inst.stop() {
                tracing::warn!(target: "automation", session = %session_id, error = %e, "max_runtime stop failed");
            }
        } else {
            // Session is no longer running; the completion loop already
            // finalized it. Skip overwriting the recorded outcome.
            continue;
        }

        store
            .update(|list| {
                if let Some(a) = list.iter_mut().find(|a| a.id == auto_id) {
                    if let Some(run) = a.state.last_run.as_mut() {
                        if run.session_id == session_id {
                            run.outcome = crate::automation::model::RunOutcome::TimedOut;
                        }
                    }
                }
                Ok(())
            })
            .ok();
        tracing::warn!(target: "automation", id = %&auto_id[..8.min(auto_id.len())], session = %session_id, "run exceeded max_runtime; stopped");
    }
}

/// Subscribe to status transitions and finalize automation runs on a
/// `Running -> Idle` turn-completion for a session launched by an automation.
#[cfg(feature = "serve")]
pub async fn automation_completion_loop(state: Arc<crate::server::AppState>) {
    let profile = state.profile.clone();
    let store = match AutomationStore::new(&profile) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut rx = state.status_tx.subscribe();
    loop {
        let change = tokio::select! {
            r = rx.recv() => match r {
                Ok(c) => c,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            _ = state.shutdown.cancelled() => return,
        };
        let finished_turn = change.old == crate::session::Status::Running
            && change.new == crate::session::Status::Idle;
        if !finished_turn {
            continue;
        }
        // Attribute the session to an automation via its automation_id link.
        let automation_id = {
            let instances = state.instances.read().await;
            instances
                .iter()
                .find(|i| i.id == change.instance_id)
                .and_then(|i| i.automation_id.clone())
        };
        let Some(automation_id) = automation_id else {
            continue;
        };
        finalize_run(
            &store,
            &profile,
            &automation_id,
            &change.instance_id,
            change.at,
        )
        .await;
    }
}

/// Finalize a completed automation run: reset the failure counter, mark the
/// run Completed, and for fresh-mode runs archive the finished session plus any
/// runs beyond `keep_last`. For persistent-mode runs with a pending fire, clear
/// the deferred flag.
#[cfg(feature = "serve")]
async fn finalize_run(
    store: &AutomationStore,
    profile: &str,
    automation_id: &str,
    session_id: &str,
    transition_at: DateTime<Utc>,
) {
    let default_keep = crate::session::config::load_config()
        .ok()
        .flatten()
        .unwrap_or_default()
        .automation
        .default_keep_last;

    // Update automation state and read back the mode + retention so the
    // session-side archive/prune can run outside the automations lock. For a
    // persistent run with a deferred fire, also read back the title + prompt so
    // the re-injection (tmux IO) happens after the lock is released.
    let plan = store
        .update(|list| {
            let Some(a) = list.iter_mut().find(|a| a.id == automation_id) else {
                return Ok(None);
            };
            // ADR-0003: only finalize on the first Running -> Idle AFTER the
            // initial prompt was injected. A real REPL can flicker
            // Running -> Idle during startup (banner) before injection; gating
            // on injected_at prevents archiving the run before it does any work.
            let injected_at = a
                .state
                .last_run
                .as_ref()
                .filter(|r| r.session_id == session_id)
                .and_then(|r| r.injected_at);
            if !crate::automation::model::should_finalize(injected_at, transition_at) {
                return Ok(None);
            }
            a.state.consecutive_failures = 0;
            // Transition the in-flight run to Completed on normal completion. A
            // TimedOut run (set by enforce_max_runtime) is already terminal and
            // its session is no longer running, so we will not reach here for it.
            if let Some(run) = a.state.last_run.as_mut() {
                if run.session_id == session_id
                    && matches!(run.outcome, crate::automation::model::RunOutcome::Running)
                {
                    run.outcome = crate::automation::model::RunOutcome::Completed;
                }
            }
            let reinject = if a.session_mode == crate::automation::model::SessionMode::Persistent
                && a.state.pending_fire
            {
                // Clear the deferred fire and capture what to re-send.
                a.state.pending_fire = false;
                Some((format!("{} (auto)", a.name), a.spec.initial_prompt.clone()))
            } else {
                None
            };
            let keep = if a.retention.keep_last > 0 {
                a.retention.keep_last
            } else {
                default_keep
            };
            Ok(Some((a.session_mode.clone(), keep, reinject)))
        })
        .ok()
        .flatten();

    let Some((mode, keep, reinject)) = plan else {
        return;
    };

    if mode == crate::automation::model::SessionMode::Persistent {
        // Re-inject the coalesced deferred fire into the still-live persistent
        // session now that it has gone Idle. Lock is already released.
        if let Some((title, prompt)) = reinject {
            let sid = session_id.to_string();
            let res = tokio::task::spawn_blocking(move || {
                crate::automation::dispatch::inject_terminal_prompt(&sid, &title, &prompt)
            })
            .await;
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(target: "automation", session = %session_id, error = %e, "deferred re-injection failed")
                }
                Err(e) => {
                    tracing::warn!(target: "automation", session = %session_id, error = %e, "deferred re-injection task panicked")
                }
            }
        }
        return;
    }

    // Fresh mode: archive this run's session, then archive any prior fresh-run
    // sessions for this automation beyond keep_last (newest-first).
    let storage = match crate::session::Storage::new_unwatched(profile) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(target: "automation", error = %e, "finalize_run: cannot open storage");
            return;
        }
    };

    let prune = storage
        .update(|all, _groups| {
            // Archive the just-finished session.
            if let Some(inst) = all.iter_mut().find(|i| i.id == session_id) {
                inst.archive();
            }
            // Gather this automation's run sessions newest-first by created_at.
            let mut runs: Vec<&Instance> = all
                .iter()
                .filter(|i| i.automation_id.as_deref() == Some(automation_id))
                .collect();
            runs.sort_by_key(|i| std::cmp::Reverse(i.created_at));
            let ids: Vec<String> = runs.iter().map(|i| i.id.clone()).collect();
            Ok(runs_to_prune(&ids, keep))
        })
        .unwrap_or_default();

    if prune.is_empty() {
        return;
    }

    // SAFETY: archive (not hard-delete) the pruned sessions. Hard delete in an
    // autonomous loop is destructive and the safe-removal + worktree-cleanup
    // path is out of scope here; the worktree-cleanup-on-prune gap is reported
    // as a concern in the task report.
    storage
        .update(|all, _groups| {
            for inst in all.iter_mut().filter(|i| prune.iter().any(|p| p == &i.id)) {
                inst.archive();
            }
            Ok(())
        })
        .ok();
    tracing::info!(target: "automation", id = %&automation_id[..8.min(automation_id.len())], pruned = prune.len(), "archived pruned fresh-mode runs");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::model::{Automation, LaunchSpec, Trigger};
    use chrono::{Duration, Utc};

    fn auto(expr: &str) -> Automation {
        let spec = LaunchSpec {
            project_path: "/tmp".into(),
            group_path: String::new(),
            tool: Some("claude".into()),
            command: None,
            extra_args: String::new(),
            view: crate::session::View::Terminal,
            worktree_branch: None,
            sandbox: false,
            auto_approve: true,
            max_runtime_secs: 1800,
            initial_prompt: "x".into(),
            agent_name: None,
            agent_model: None,
        };
        Automation::new("a", spec, Trigger::Cron { expr: expr.into() })
    }

    // Regression: a failed run must advance next_fire to the next cron slot,
    // otherwise the automation re-fires on every poll tick (found via live
    // verification) and exhausts the consecutive-failure budget in seconds.
    #[cfg(feature = "serve")]
    #[tokio::test]
    async fn record_failure_advances_next_fire() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::automation::store::AutomationStore::with_path(
            tmp.path().join("automations.json"),
        );
        let mut a = auto("* * * * *");
        let id = a.id.clone();
        let stale = Utc::now() - Duration::minutes(10);
        a.state.next_fire = Some(stale);
        store
            .update(|list| {
                list.push(a.clone());
                Ok(())
            })
            .unwrap();

        record_failure(&store, &id, "default", "boom".into()).await;

        let reloaded = store.load().unwrap();
        let st = &reloaded[0].state;
        assert_eq!(st.consecutive_failures, 1);
        let next = st.next_fire.expect("next_fire must be set after a failure");
        assert!(
            next > stale,
            "next_fire must advance past the stale fire time, got {next}"
        );
    }

    #[test]
    fn missing_next_fire_counts_as_due_then_initializes() {
        let now = Utc::now();
        let mut a = auto("* * * * *");
        assert_eq!(due_automations(std::slice::from_ref(&a), now), vec![0]);
        ensure_next_fire(&mut a, now);
        assert!(a.state.next_fire.unwrap() > now);
    }

    #[test]
    fn disabled_is_never_due() {
        let now = Utc::now();
        let mut a = auto("* * * * *");
        a.enabled = false;
        a.state.next_fire = Some(now - Duration::minutes(1));
        assert!(due_automations(std::slice::from_ref(&a), now).is_empty());
    }

    fn running(ids: &[&str]) -> std::collections::HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn persistent_collision_true_when_id_running() {
        let mut a = auto("* * * * *");
        a.session_mode = crate::automation::model::SessionMode::Persistent;
        a.state.persistent_session_id = Some("sess-1".into());
        assert!(persistent_collision(&a, &running(&["sess-1", "sess-2"])));
    }

    #[test]
    fn persistent_collision_false_for_fresh_mode() {
        let mut a = auto("* * * * *");
        // Fresh is the default, but a stray id must still not collide.
        a.state.persistent_session_id = Some("sess-1".into());
        assert!(!persistent_collision(&a, &running(&["sess-1"])));
    }

    #[test]
    fn persistent_collision_false_when_no_persistent_id() {
        let mut a = auto("* * * * *");
        a.session_mode = crate::automation::model::SessionMode::Persistent;
        assert!(a.state.persistent_session_id.is_none());
        assert!(!persistent_collision(&a, &running(&["sess-1"])));
    }

    #[test]
    fn persistent_collision_false_when_id_not_running() {
        let mut a = auto("* * * * *");
        a.session_mode = crate::automation::model::SessionMode::Persistent;
        a.state.persistent_session_id = Some("sess-9".into());
        assert!(!persistent_collision(&a, &running(&["sess-1", "sess-2"])));
    }

    #[test]
    fn runs_to_prune_keeps_newest_n() {
        let sessions = vec![
            "s5".to_string(),
            "s4".into(),
            "s3".into(),
            "s2".into(),
            "s1".into(),
        ];
        let prune = runs_to_prune(&sessions, 3);
        assert_eq!(prune, vec!["s2".to_string(), "s1".into()]);
    }

    #[test]
    fn runs_to_prune_noop_when_under_limit() {
        let sessions = vec!["s2".to_string(), "s1".into()];
        assert!(runs_to_prune(&sessions, 5).is_empty());
    }
}
