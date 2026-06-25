use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::automation::model::Automation;
#[cfg(feature = "serve")]
use crate::session::View;
use crate::session::{Instance, Storage};

/// Result of dispatching a run: the session id, plus when the initial prompt was
/// injected (`None` if injection has not happened yet, e.g. a structured-view
/// fresh launch whose worker the ACP reconciler spawns later). The completion
/// loop gates finalization on this (ADR-0003).
pub struct Dispatched {
    pub session_id: String,
    pub injected_at: Option<DateTime<Utc>>,
}

/// Pure mapping from an automation's launch spec onto a fresh Instance.
pub fn instance_from_spec(automation: &Automation, profile: &str) -> Instance {
    let spec = &automation.spec;
    let title = format!("{} (auto)", automation.name);
    let mut inst = Instance::new(&title, &spec.project_path);
    inst.source_profile = profile.to_string();
    inst.group_path = spec.group_path.clone();
    if let Some(tool) = &spec.tool {
        inst.tool = tool.clone();
    }
    if let Some(cmd) = &spec.command {
        inst.command = cmd.clone();
    }
    inst.extra_args = spec.extra_args.clone();
    #[cfg(feature = "serve")]
    {
        inst.view = spec.view;
        inst.agent_name = spec.agent_name.clone();
        inst.agent_model = spec.agent_model.clone();
    }
    inst.yolo_mode = spec.auto_approve;
    inst.initial_prompt = spec.initial_prompt.clone();
    inst.automation_id = Some(automation.id.clone());
    inst
}

/// Send a prompt into an already-running terminal-view session by its id.
/// Used for persistent-mode re-injection (reuse path and deferred-fire
/// completion). The session must already exist in tmux.
pub fn inject_terminal_prompt(session_id: &str, title: &str, prompt: &str) -> Result<()> {
    let session = crate::tmux::Session::new(session_id, title)?;
    session.send_keys(prompt)?;
    Ok(())
}

/// Build, persist, launch, and inject the initial prompt. Returns the new session id.
///
/// For terminal-view runs this spawns the tmux session synchronously (via
/// `spawn_blocking`) and then injects the initial prompt.  Structured-view
/// workers are spawned by the ACP reconciler; the caller (Task 7 scheduler)
/// is responsible for sending the prompt once the worker is up.
pub async fn launch_run(automation: &Automation, profile: &str) -> Result<Dispatched> {
    use crate::automation::model::SessionMode;

    // Persistent mode with a recorded session that still exists in storage:
    // reuse it instead of creating a new session, so context carries across
    // runs (ADR-0003). Anything else (fresh mode, or a persistent automation
    // whose recorded session is gone) falls through to a fresh launch.
    if automation.session_mode == SessionMode::Persistent {
        if let Some(sid) = automation.state.persistent_session_id.clone() {
            let storage = Storage::new_unwatched(profile)?;
            let existing = storage.load()?.into_iter().find(|i| i.id == sid);
            if let Some(inst) = existing {
                return reuse_persistent(inst, &automation.spec.initial_prompt).await;
            }
        }
    }

    fresh_launch(automation, profile).await
}

/// Reuse a persistent automation's existing session: relaunch its tmux session
/// if it has stopped, then inject the prompt. Returns the session id.
async fn reuse_persistent(mut inst: Instance, prompt: &str) -> Result<Dispatched> {
    let id = inst.id.clone();

    // Structured-view persistent reuse is a documented v1 limitation: the v1
    // CLI only creates terminal automations, so this path is unreachable in
    // practice. Skip re-injection rather than mis-send into an ACP worker.
    #[cfg(feature = "serve")]
    if !matches!(inst.view, View::Terminal) {
        tracing::warn!(
            target: "automation",
            session = %id,
            "persistent reuse of a structured-view session is not supported in v1; skipping re-injection",
        );
        return Ok(Dispatched {
            session_id: id,
            injected_at: None,
        });
    }

    let title = inst.title.clone();
    let prompt = prompt.to_string();
    let id_for_inject = id.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let session = crate::tmux::Session::new(&inst.id, &inst.title)?;
        if !session.exists() {
            inst.start_with_size(None)?;
            // Wait for REPL to be ready, matching the fresh-inject readiness wait.
            std::thread::sleep(std::time::Duration::from_millis(600));
        }
        inject_terminal_prompt(&id_for_inject, &title, &prompt)?;
        Ok(())
    })
    .await??;
    Ok(Dispatched {
        session_id: id,
        injected_at: Some(Utc::now()),
    })
}

/// Create a brand-new session for this run (fresh mode, or persistent first
/// fire), persist it, launch, and inject the initial prompt. Returns the id.
async fn fresh_launch(automation: &Automation, profile: &str) -> Result<Dispatched> {
    let mut inst = instance_from_spec(automation, profile);
    let id = inst.id.clone();

    let storage = Storage::new_unwatched(profile)?;
    storage.update(|all, _groups| {
        all.push(inst.clone());
        Ok(())
    })?;

    // For structured-view runs the ACP reconciler spawns the worker; the
    // caller (Task 7 scheduler) sends the prompt once the worker is up.
    // Without the `serve` feature every session is terminal-mode.
    #[cfg(not(feature = "serve"))]
    let is_terminal = true;
    #[cfg(feature = "serve")]
    let is_terminal = matches!(inst.view, View::Terminal);

    if is_terminal {
        tokio::task::spawn_blocking(move || -> Result<()> {
            inst.start_with_size(None)?;
            inst.inject_initial_prompt()?;
            Ok(())
        })
        .await??;
        return Ok(Dispatched {
            session_id: id,
            injected_at: Some(Utc::now()),
        });
    }
    // Structured-view: injection happens later, so leave injected_at unset.
    Ok(Dispatched {
        session_id: id,
        injected_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::model::{Automation, LaunchSpec, Trigger};

    fn spec() -> LaunchSpec {
        LaunchSpec {
            project_path: "/tmp/proj".into(),
            group_path: "team".into(),
            tool: Some("claude".into()),
            command: None,
            extra_args: "--foo".into(),
            view: crate::session::View::Terminal,
            worktree_branch: None,
            sandbox: false,
            auto_approve: true,
            max_runtime_secs: 1800,
            initial_prompt: "summarize slack".into(),
            agent_name: None,
            agent_model: None,
        }
    }

    #[test]
    fn instance_from_spec_maps_fields() {
        let a = Automation::new(
            "slack",
            spec(),
            Trigger::Cron {
                expr: "* * * * *".into(),
            },
        );
        let inst = instance_from_spec(&a, "default");
        assert_eq!(inst.project_path, "/tmp/proj");
        assert_eq!(inst.group_path, "team");
        assert_eq!(inst.extra_args, "--foo");
        assert!(inst.yolo_mode, "auto_approve must map to yolo_mode");
        assert_eq!(inst.initial_prompt, "summarize slack");
        assert!(inst.title.contains("slack"));
        assert_eq!(inst.source_profile, "default");
        assert_eq!(inst.automation_id.as_deref(), Some(a.id.as_str()));
    }
}
