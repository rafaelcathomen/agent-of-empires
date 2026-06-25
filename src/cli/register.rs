//! `aoe register` command implementation.
//!
//! Adopts an already-running tmux session — one a user started in their own
//! terminal (e.g. `tmux new -s mywork; claude`) — into aoe as a managed
//! session rendered from its raw tmux pane (the terminal view, as opposed to
//! the ACP structured view; unrelated to a session's optional paired
//! `terminal_info` pane). The session then shows up in `aoe list`, the TUI
//! dashboard, and is attachable via `aoe session attach`, exactly like a
//! session aoe created itself.
//!
//! aoe derives a session's tmux name deterministically from its instance id
//! and title (`aoe_<title>_<id8>`; see `tmux::Session::generate_name`). So the
//! adoption strategy is: mint an `Instance`, `rename-session` the user's tmux
//! session to that derived name, stamp `AOE_INSTANCE_ID` into the session's
//! hidden env, apply the usual tmux status-bar options, and persist the
//! instance. After that every existing aoe code path (attach, status polling,
//! diff view, …) treats it like any other session because the tmux name and
//! env match the convention. See issues #1056 and #2276.
//!
//! Status hooks: aoe normally injects `AOE_INSTANCE_ID` into the agent's
//! process env at launch (`AOE_INSTANCE_ID=<id> claude`), which the installed
//! status hooks read. An adopted agent was started by the user, so its process
//! env lacks that var and it can't be injected after the fact. To bridge this,
//! the host hooks fall back to reading the id from the tmux session's hidden
//! env (stamped here) when their process env is empty (see
//! `hooks::hook_command_with_base` / `hook_command_session_id_host`). Hooks
//! still need to be present in the agent's config (true for any prior aoe
//! user); absent hooks, status falls back to pane-content detection, which
//! needs neither env nor hooks.

use anyhow::{bail, Context, Result};
use clap::Args;
use std::path::PathBuf;

use crate::session::{GroupTree, Instance, Status, Storage};

#[derive(Args)]
pub struct RegisterArgs {
    /// Name of the existing tmux session to adopt (as shown by `tmux ls`).
    /// The agent must already be running inside this tmux session.
    tmux_session: String,

    /// Session title (defaults to a readable form of the tmux session name)
    #[arg(short = 't', long)]
    title: Option<String>,

    /// Project directory for the session (defaults to the tmux pane's current
    /// working directory)
    #[arg(long)]
    path: Option<PathBuf>,

    /// Agent tool running in the session (e.g. claude, codex, gemini).
    /// Defaults to auto-detecting from the pane's foreground command, falling
    /// back to `claude` when detection is inconclusive.
    #[arg(long = "tool")]
    tool: Option<String>,

    /// Group path to place the session under
    #[arg(short = 'g', long)]
    group: Option<String>,
}

#[tracing::instrument(target = "cli.register", skip_all, fields(profile = %profile))]
pub async fn run(profile: &str, args: RegisterArgs) -> Result<()> {
    let raw_name = args.tmux_session.trim().to_string();
    if raw_name.is_empty() {
        bail!("A tmux session name is required: aoe register <tmux-session>");
    }

    // Reject sessions aoe already owns. Adopting an aoe-prefixed session would
    // double-register it (and `aoe_*` names round-trip through our own
    // creation path, not this one).
    if is_aoe_owned(&raw_name) {
        bail!(
            "'{raw_name}' is already an aoe-managed tmux session; nothing to register.\n\
             Pass the name of a session you started yourself outside aoe."
        );
    }

    let session = crate::tmux::Session::from_name(&raw_name);
    if !session.exists() {
        let candidates = list_adoptable_sessions();
        let hint = if candidates.is_empty() {
            "No adoptable tmux sessions found. Start your agent inside tmux first, e.g.:\n  \
             tmux new -s mywork\n  claude        # run your agent in that session\n\
             then: aoe register mywork"
                .to_string()
        } else {
            format!(
                "Adoptable tmux sessions:\n{}",
                candidates
                    .iter()
                    .map(|s| format!("  {s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        bail!("No tmux session named '{raw_name}'.\n{hint}");
    }

    // Guard against re-registering. A session aoe already adopted carries the
    // AOE_INSTANCE_ID marker in its hidden env.
    if let Some(existing_id) =
        crate::tmux::env::get_hidden_env(&raw_name, crate::tmux::env::AOE_INSTANCE_ID_KEY)
    {
        bail!("tmux session '{raw_name}' is already registered with aoe (instance {existing_id}).");
    }

    // Resolve the project path: explicit flag, else the pane's cwd.
    let path = match &args.path {
        Some(p) => p.clone(),
        None => {
            let p = crate::tmux::utils::pane_current_path(&raw_name).ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not determine the working directory of tmux session '{raw_name}'. \
                     Pass --path <dir>."
                )
            })?;
            PathBuf::from(p)
        }
    };
    if !path.is_dir() {
        bail!("Path is not a directory: {}", path.display());
    }
    let path = path
        .canonicalize()
        .with_context(|| format!("Failed to resolve path: {}", path.display()))?;
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Project path is not valid UTF-8: {}", path.display()))?
        .to_string();

    let title = resolve_title(args.title.as_deref(), &raw_name);

    let pane_cmd = crate::tmux::utils::pane_current_command(&raw_name);
    let tool = resolve_tool(args.tool.as_deref(), pane_cmd.as_deref())?;

    let config = crate::session::repo_config::resolve_config_with_repo_or_warn(profile, &path);
    let detect_as = config
        .session
        .agent_detect_as
        .get(&tool)
        .cloned()
        .unwrap_or_default();

    let storage = Storage::new_unwatched(profile)?;
    let (instances, _groups) = storage.load_with_groups()?;
    if crate::cli::add::is_duplicate_session(&instances, &title, &path_str) {
        bail!(
            "A session titled '{title}' already exists at {path_str}.\n\
             Tip: pass --title to give the adopted session a distinct name."
        );
    }

    let mut instance = Instance::new(&title, &path_str);
    instance.source_profile = profile.to_string();
    instance.tool = tool.clone();
    instance.detect_as = detect_as;
    // The agent is already live; show it as running. The status poller
    // reconciles to the true state (waiting/idle/…) on its next tick.
    instance.status = Status::Running;
    if let Some(group) = &args.group {
        instance.group_path = group.trim().to_string();
    }

    // Rename the user's tmux session to aoe's convention so every existing
    // attach/probe/status path finds it by the derived name.
    let new_name = crate::tmux::Session::generate_name(&instance.id, &instance.title);
    session
        .rename(&new_name)
        .with_context(|| format!("Failed to rename tmux session '{raw_name}' -> '{new_name}'"))?;

    // `Session::rename` is a no-op when its source no longer exists, so a
    // successful return does NOT prove we renamed anything: the original
    // session may have exited, or a concurrent `aoe register` may have already
    // adopted (renamed) it in the cache-staleness window of the guard above.
    // Confirm the destination is actually live before committing an instance
    // for it; `exists()` does a real `tmux has-session` for this fresh name.
    if !crate::tmux::Session::from_name(&new_name).exists() {
        bail!(
            "tmux session '{raw_name}' could not be adopted: it exited or was registered by \
             another process before the rename landed. No instance was created."
        );
    }

    // Link the renamed session back to the instance, then apply the same tmux
    // status-bar options a freshly-launched session gets. The AOE_INSTANCE_ID
    // stamp is load-bearing (status hooks + aoe's session->instance reverse
    // lookup), so surface a failure to the user rather than only logging it.
    if let Err(e) = crate::tmux::env::set_hidden_env(
        &new_name,
        crate::tmux::env::AOE_INSTANCE_ID_KEY,
        &instance.id,
    ) {
        tracing::warn!(
            target: "cli.register",
            session = %new_name,
            "Failed to set AOE_INSTANCE_ID on adopted session: {e}"
        );
        eprintln!(
            "⚠ Could not stamp AOE_INSTANCE_ID on '{new_name}': {e}\n  \
             Status hooks won't report for this session; aoe falls back to \
             pane-content status detection."
        );
    }
    let branch = instance.worktree_info.as_ref().map(|w| w.branch.clone());
    crate::tmux::status_bar::apply_all_tmux_options(
        &new_name,
        &instance.title,
        branch.as_deref(),
        None,
    );

    // Install the agent's status hooks into its host config. A natively
    // launched session does this during launch; adoption skips the launch
    // path, so without this a first-time user's adopted agent would have no
    // hooks and the tmux-env fallback would have nothing to feed. Idempotent
    // and gated on the `session.agent_status_hooks` config toggle.
    instance.install_status_hooks();

    let persisted = storage.update(|all_instances, groups| {
        if crate::cli::add::is_duplicate_session(
            all_instances,
            &instance.title,
            &instance.project_path,
        ) {
            return Ok(false);
        }
        all_instances.push(instance.clone());
        if !instance.group_path.is_empty() {
            let mut group_tree = GroupTree::new_with_groups(all_instances, groups);
            group_tree.create_group(&instance.group_path);
            *groups = group_tree.get_all_groups();
        }
        Ok(true)
    })?;

    if !persisted {
        // Lost a race with a concurrent writer. Undo the rename so the user's
        // session keeps its original name rather than being left orphaned
        // under the aoe prefix with no backing instance.
        let renamed = crate::tmux::Session::from_name(&new_name);
        let _ = renamed.rename(&raw_name);
        bail!(
            "A session titled '{}' at {} was created concurrently; registration aborted.",
            instance.title,
            instance.project_path
        );
    }

    let short_id = super::truncate_id(&instance.id, 8);
    println!("✓ Registered tmux session '{raw_name}' with aoe");
    println!("  Title:   {}", instance.title);
    println!("  Profile: {}", storage.profile());
    println!("  Path:    {}", path.display());
    println!("  Tool:    {}", instance.tool);
    println!("  ID:      {}", instance.id);
    println!("  Tmux:    {new_name} (renamed from '{raw_name}')");
    println!();
    println!("Next steps:");
    println!("  aoe list                       # see it in the session list");
    println!("  aoe session attach {short_id}      # attach to it");
    println!("  aoe                            # open the TUI dashboard");

    Ok(())
}

/// tmux sessions not owned by aoe — candidates for `aoe register`. Best-effort:
/// a failed `tmux ls` yields an empty list (the caller already knows the named
/// session does not exist).
fn list_adoptable_sessions() -> Vec<String> {
    let output = std::process::Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|s| s.to_string())
            .filter(|s| !is_aoe_owned(s))
            .collect(),
        _ => Vec::new(),
    }
}

/// True for any tmux session aoe owns, across debug (`aoe_dev_*`) and release
/// (`aoe_*`) builds — both share the `aoe_` stem, so one check covers a debug
/// binary running alongside a release install. Deliberately broad: better to
/// keep an `aoe_`-looking session out of adoption than to risk double-managing
/// a live session.
fn is_aoe_owned(name: &str) -> bool {
    name.starts_with("aoe_")
}

/// Resolve the session title: an explicit `--title` wins, but only when it is
/// non-blank after trimming — a blank value falls back to the readable form of
/// the tmux name rather than producing an empty title (which would render blank
/// in `aoe list`/TUI and yield an `aoe__<id>` tmux name).
fn resolve_title(explicit: Option<&str>, raw_name: &str) -> String {
    explicit
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .unwrap_or_else(|| title_from_tmux_name(raw_name))
}

/// Turn a raw tmux session name into a readable default title by replacing
/// `_`/`-` separators with spaces and collapsing whitespace. Empty input
/// falls back to `"session"`.
fn title_from_tmux_name(name: &str) -> String {
    let spaced: String = name
        .chars()
        .map(|c| if c == '_' || c == '-' { ' ' } else { c })
        .collect();
    let title = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        "session".to_string()
    } else {
        title
    }
}

/// Resolve the agent tool for an adopted session. An explicit `--tool` wins
/// (validated against the known agents); otherwise the pane's foreground
/// command is matched against known agent names/aliases, defaulting to
/// `claude` when that is inconclusive (e.g. Claude Code often shows up as
/// `node`).
fn resolve_tool(explicit: Option<&str>, pane_cmd: Option<&str>) -> Result<String> {
    if let Some(t) = explicit {
        let t = t.trim();
        // Explicit input is matched strictly (exact canonical name or alias),
        // not by the lenient substring matcher `resolve_tool_name` uses for
        // free-form command strings — otherwise `--tool not-an-agent` would
        // sneak through on an incidental substring like "agent".
        if let Some(name) = crate::agents::agent_names().into_iter().find(|name| {
            crate::agents::get_agent(name).is_some_and(|a| a.name == t || a.aliases.contains(&t))
        }) {
            return Ok(name.to_string());
        }
        bail!(
            "Unknown tool '{t}'.\nSupported tools: {}",
            crate::agents::agent_names().join(", ")
        );
    }
    Ok(pane_cmd
        .and_then(crate::agents::resolve_tool_name)
        .unwrap_or("claude")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_from_tmux_name_replaces_separators() {
        assert_eq!(title_from_tmux_name("my_work"), "my work");
        assert_eq!(title_from_tmux_name("foo-bar_baz"), "foo bar baz");
    }

    #[test]
    fn title_from_tmux_name_plain_passes_through() {
        assert_eq!(title_from_tmux_name("mywork"), "mywork");
    }

    #[test]
    fn title_from_tmux_name_empty_falls_back() {
        assert_eq!(title_from_tmux_name(""), "session");
        assert_eq!(title_from_tmux_name("__"), "session");
    }

    #[test]
    fn resolve_title_uses_explicit_when_non_empty() {
        assert_eq!(resolve_title(Some("My Agent"), "sess_x"), "My Agent");
    }

    #[test]
    fn resolve_title_falls_back_when_explicit_blank_or_absent() {
        // A blank/whitespace --title must NOT produce an empty title (which
        // would render blank in `aoe list` and yield an `aoe__<id>` tmux name).
        assert_eq!(resolve_title(Some(""), "my_sess"), "my sess");
        assert_eq!(resolve_title(Some("   "), "my_sess"), "my sess");
        assert_eq!(resolve_title(None, "my_sess"), "my sess");
    }

    #[test]
    fn resolve_tool_explicit_known_wins() {
        assert_eq!(
            resolve_tool(Some("codex"), Some("claude")).unwrap(),
            "codex"
        );
        assert_eq!(resolve_tool(Some("claude"), None).unwrap(), "claude");
    }

    #[test]
    fn resolve_tool_explicit_unknown_errors() {
        // "definitely-not-an-agent" contains the substring "agent"; strict
        // matching must still reject it.
        assert!(resolve_tool(Some("definitely-not-an-agent"), None).is_err());
        assert!(resolve_tool(Some("myclaude"), None).is_err());
    }

    #[test]
    fn is_aoe_owned_covers_debug_and_release_prefixes() {
        assert!(is_aoe_owned("aoe_My_Proj_abc12345")); // release
        assert!(is_aoe_owned("aoe_dev_My_Proj_abc12345")); // debug
        assert!(is_aoe_owned("aoe_term_x"));
        assert!(!is_aoe_owned("mywork"));
        assert!(!is_aoe_owned("vault"));
    }

    #[test]
    fn resolve_tool_explicit_alias_resolves_to_canonical() {
        // "open-code" is a registered alias of the canonical "opencode".
        assert_eq!(resolve_tool(Some("open-code"), None).unwrap(), "opencode");
    }

    #[test]
    fn resolve_tool_detects_from_pane_command() {
        assert_eq!(resolve_tool(None, Some("codex")).unwrap(), "codex");
    }

    #[test]
    fn resolve_tool_defaults_to_claude_when_inconclusive() {
        // A bare `node` process (how Claude Code often appears) is not a known
        // agent token, so detection falls back to claude.
        assert_eq!(resolve_tool(None, Some("node")).unwrap(), "claude");
        assert_eq!(resolve_tool(None, None).unwrap(), "claude");
    }
}
