use anyhow::Result;

/// Ensure a daemon (the scheduler host) is running. Returns true if this call
/// spawned one. Auto-spawn is the fallback durability path; the consent-gated
/// OS-service install lives in the dashboard plan (ADR-0001).
#[cfg(feature = "serve")]
pub fn ensure_scheduler_running(profile: &str) -> Result<bool> {
    if crate::cli::serve::daemon_pid().is_some() {
        return Ok(false);
    }
    crate::cli::serve::ensure_daemon_spawned(profile)?;
    Ok(true)
}

/// No-op stub when the `serve` feature is not compiled in. Returns `Ok(false)`
/// so callers compile without conditionals.
#[cfg(not(feature = "serve"))]
pub fn ensure_scheduler_running(_profile: &str) -> Result<bool> {
    Ok(false)
}
