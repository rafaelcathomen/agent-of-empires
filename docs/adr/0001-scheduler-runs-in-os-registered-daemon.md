# Scheduler runs inside an OS-registered AoE daemon

Scheduled jobs must fire even when no TUI is open, so the scheduler runs inside the
existing `aoe serve` daemon (in a scheduler-only mode that need not open the web port),
and that daemon is registered as an OS-level service (systemd user unit on Linux,
launchd LaunchAgent on macOS) so it survives logout and reboot. We rejected
"require the user to run `aoe serve`" (silently fails for TUI-only users, the common
case) and a bespoke second mini-daemon (duplicates lifecycle machinery already in
`src/server`/`src/cli/serve.rs`).

## Consequences

- Auto-spawning the daemon on first enabled job is the *fallback* when the OS service
  is not installed: it works for the current login session but does not survive reboot.
- The honest limitation remains that nothing fires while the daemon is down; missed
  fires are skipped, not caught up (see the no-catch-up decision).
