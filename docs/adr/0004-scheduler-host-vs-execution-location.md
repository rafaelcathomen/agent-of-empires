# Scheduler host is independent of run execution location

Where the Scheduler Daemon runs and where a run executes are independent axes. The
single, local Scheduler Daemon (ADR-0001) evaluates triggers and dispatches runs; each
run executes wherever its Launch Spec's **execution location** points — on the local
host, in a local Docker sandbox, or (future) on a remote host over SSH. Remote is a
third value on the execution-location axis the launch path already has
(`build_host_command` vs the container command), not a new concept type, so automations
inherit remote execution for free once the launch path supports it.

We chose "local daemon orchestrates remote runs" over running the scheduler on the
remote host. Consequence: the local machine must be on at fire time for a remote run to
fire. Running the same daemon binary as an OS service on an always-on remote host
(so automations fire with the laptop off) stays reachable with no architectural change —
it is just "install the service there" — but is explicitly out of scope and undecided.

This reinforces ADR-0001 (one durable orchestrating daemon) rather than contradicting it.
