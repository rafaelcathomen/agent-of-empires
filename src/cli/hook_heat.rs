//! Hidden `aoe __hook-heat` subcommand.
//!
//! Machine-spawned by each agent's user-prompt hook (Claude/Cursor/Qwen/Codex
//! `UserPromptSubmit`, Gemini `BeforeAgent`) to bump the per-session heat
//! accumulator. Reads no stdin and takes no arguments: the bump's timestamp is
//! the wall clock at hook-fire time, and the decay-and-add math lives in
//! `crate::hooks::heat`.
//!
//! Always exits 0: a non-zero hook blocks the agent. Errors surface through
//! `tracing::debug!`.

use anyhow::Result;
use clap::Args;

#[derive(Args)]
pub struct HookHeatArgs {}

pub async fn run(_args: HookHeatArgs) -> Result<()> {
    let Ok(instance_id) = std::env::var("AOE_INSTANCE_ID") else {
        return Ok(());
    };
    if let Err(e) = crate::session::validate_instance_id(&instance_id) {
        tracing::debug!(target: "hooks.heat", "rejecting unsafe AOE_INSTANCE_ID: {e}");
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Err(e) = crate::hooks::bump_heat_via_guard(&instance_id, now) {
        tracing::debug!(target: "hooks.heat", "heat hook failed: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::hooks::test_support::BaseGuard;
    use crate::hooks::{bump_heat_via_guard, read_hook_heat};

    #[test]
    #[serial_test::serial(hook_base)]
    fn prompt_creates_accumulator() {
        let (_g, _base, _tmp) = BaseGuard::ready();
        bump_heat_via_guard("heat_cli_one", 5_000_000).unwrap();
        let acc = read_hook_heat("heat_cli_one").unwrap();
        assert!((acc.s - 1.0).abs() < 1e-9);
        assert_eq!(acc.t_last, 5_000_000);
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn burst_then_spread_increments() {
        let (_g, _base, _tmp) = BaseGuard::ready();
        // Two prompts 10s apart: the second is burst-clamped near the floor.
        bump_heat_via_guard("heat_cli_burst", 6_000_000).unwrap();
        bump_heat_via_guard("heat_cli_burst", 6_000_010).unwrap();
        let burst = read_hook_heat("heat_cli_burst").unwrap().s;
        // Two prompts a full window apart: the second adds the full increment.
        bump_heat_via_guard("heat_cli_spread", 6_000_000).unwrap();
        bump_heat_via_guard("heat_cli_spread", 6_000_120).unwrap();
        let spread = read_hook_heat("heat_cli_spread").unwrap().s;
        assert!(
            spread > burst,
            "spread {spread} should exceed burst {burst}"
        );
    }

    #[test]
    fn unsafe_instance_id_is_rejected() {
        assert!(crate::session::validate_instance_id("../etc").is_err());
        assert!(crate::session::validate_instance_id("").is_err());
    }
}
