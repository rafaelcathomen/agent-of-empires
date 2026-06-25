# Run lifecycle and firing semantics

A **run** is one execution of a job. Its lifecycle and firing rules:

- **Completion (fresh mode):** a run is complete on the first `Running -> Idle` transition
  *after* the initial prompt is injected (the agent's first turn ending), reusing the
  daemon's existing transition detection. A per-job `max_runtime` (default 30m) caps a
  wedged or looping run: on expiry the run is stopped, marked timed-out, and surfaced.
- **Completion (persistent mode):** the session is reused across runs; it is not archived.
- **Busy collision (persistent mode):** if a fire is due while the session is busy
  (`Running`, or a human is interacting), do not inject. Mark a single fire pending and
  inject when the session next returns to `Idle`. Multiple fires missed while busy
  coalesce to one pending run; we never interrupt an in-flight turn or stomp a human.
- **No catch-up:** fires missed while the daemon was down are skipped, not replayed
  (consistent with Codex local and Claude Code session tasks).

We rejected agent-signalled completion (needs every prompt to cooperate; brittle) and
interrupt-and-inject on collision (destroys in-flight work).
