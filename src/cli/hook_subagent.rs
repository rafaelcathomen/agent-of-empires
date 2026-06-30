//! Hidden `aoe __hook-subagent --delta <n>` subcommand.
//!
//! Machine-spawned by the Claude Code PreToolUse (`--delta 1`) and
//! SubagentStop (`--delta -1`) hooks to track how many Task subagents are
//! running. The increment path re-reads the hook stdin JSON to confirm
//! `tool_name == "Task"` so the always-firing PreToolUse (matcher `None`)
//! only counts subagents; the decrement path runs unconditionally and the
//! writer clamps the stored counter at `>= 0`.
//!
//! Always exits 0: a non-zero hook blocks the agent. Errors surface through
//! `tracing::debug!`. Stdin is capped at 1 MiB to bound memory.

use std::io::Read;

use anyhow::Result;
use clap::Args;

const STDIN_BYTE_CAP: u64 = 1 << 20;
// See extract_session_id.rs: bounds the post-cap stdin drain so a never-closing
// stdin can't hang the hook.
const STDIN_DRAIN_CAP: u64 = 64 << 20;

#[derive(Args)]
pub struct HookSubagentArgs {
    /// Baked at hook-install time: `1` on PreToolUse(Task), `-1` on
    /// SubagentStop. `allow_hyphen_values` lets clap accept the literal `-1`.
    #[arg(long, allow_hyphen_values = true)]
    delta: i64,
}

pub async fn run(args: HookSubagentArgs) -> Result<()> {
    let Ok(instance_id) = std::env::var("AOE_INSTANCE_ID") else {
        return Ok(());
    };
    if let Err(e) = crate::session::validate_instance_id(&instance_id) {
        tracing::debug!(
            target: "hooks.subagent",
            "rejecting unsafe AOE_INSTANCE_ID: {e}"
        );
        return Ok(());
    }
    if let Err(e) = run_inner(std::io::stdin().lock(), &instance_id, args.delta) {
        tracing::debug!(target: "hooks.subagent", "subagent hook failed: {e}");
    }
    Ok(())
}

fn run_inner<R: Read>(mut stdin: R, instance_id: &str, delta: i64) -> Result<()> {
    let mut buf = String::new();
    let read_res = (&mut stdin).take(STDIN_BYTE_CAP).read_to_string(&mut buf);
    // Drain bytes past the read cap so a large tool input doesn't EPIPE the
    // agent's write when we return early; bounded by STDIN_DRAIN_CAP so a
    // never-closing stdin can't hang the hook.
    std::io::copy(
        &mut (&mut stdin).take(STDIN_DRAIN_CAP),
        &mut std::io::sink(),
    )
    .ok();
    read_res?;
    if delta > 0 {
        // Gate the increment on the actual tool name: PreToolUse fires for
        // every tool, but only Task spawns a counted subagent. This +1 path
        // is conditional (a malformed or oversized-then-truncated payload
        // returns Err and skips the bump) while the SubagentStop -1 path runs
        // unconditionally, so the two can drift; the writer's clamp(>=0) plus
        // the read-side TTL bound the worst case to an early spinner clear.
        let value: serde_json::Value = serde_json::from_str(&buf)?;
        if value.get("tool_name").and_then(|v| v.as_str()) != Some("Task") {
            return Ok(());
        }
    }
    crate::hooks::adjust_subagent_counter_via_guard(instance_id, delta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::test_support::BaseGuard;

    fn count(base: &std::path::Path, id: &str) -> i64 {
        std::fs::read_to_string(base.join(id).join("subagent_active"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn increments_on_task() {
        let (_g, base, _tmp) = BaseGuard::ready();
        run_inner(br#"{"tool_name":"Task"}"#.as_slice(), "task_inc", 1).unwrap();
        assert_eq!(count(&base, "task_inc"), 1);
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn ignores_non_task_on_increment() {
        let (_g, base, _tmp) = BaseGuard::ready();
        run_inner(br#"{"tool_name":"Bash"}"#.as_slice(), "task_skip", 1).unwrap();
        assert_eq!(count(&base, "task_skip"), 0);
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn decrement_does_not_read_stdin() {
        let (_g, base, _tmp) = BaseGuard::ready();
        run_inner(br#"{"tool_name":"Task"}"#.as_slice(), "task_dec", 1).unwrap();
        run_inner(br#"{"tool_name":"Task"}"#.as_slice(), "task_dec", 1).unwrap();
        // SubagentStop carries no tool_name; the -1 must still decrement.
        run_inner(b"".as_slice(), "task_dec", -1).unwrap();
        assert_eq!(count(&base, "task_dec"), 1);
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn decrement_clamps_at_zero() {
        let (_g, base, _tmp) = BaseGuard::ready();
        run_inner(b"".as_slice(), "task_clamp", -1).unwrap();
        assert_eq!(count(&base, "task_clamp"), 0);
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn parallel_increments_total_n() {
        let (_g, base, _tmp) = BaseGuard::ready();
        std::thread::scope(|s| {
            for _ in 0..5 {
                // The hook base is a thread-local override in tests; each
                // spawned thread must point at the same tempdir base before
                // run_inner reaches the counter writer.
                let thread_base = base.clone();
                s.spawn(move || {
                    crate::hooks::override_base_for_test(thread_base);
                    crate::hooks::reset_for_test();
                    run_inner(br#"{"tool_name":"Task"}"#.as_slice(), "task_par", 1).unwrap();
                });
            }
        });
        assert_eq!(count(&base, "task_par"), 5);
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn malformed_json_on_increment_is_noop() {
        let (_g, base, _tmp) = BaseGuard::ready();
        let err = run_inner(b"not json{{{".as_slice(), "task_bad", 1);
        assert!(err.is_err());
        assert_eq!(count(&base, "task_bad"), 0);
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn unsafe_instance_id_is_rejected() {
        // run() guards on AOE_INSTANCE_ID via validate_instance_id before
        // touching the counter; confirm the validator rejects path-escape ids.
        assert!(crate::session::validate_instance_id("../etc").is_err());
        assert!(crate::session::validate_instance_id("").is_err());
    }
}
