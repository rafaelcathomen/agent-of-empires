//! E2E coverage for the TUI Automations view.
//!
//! Seeds an automation via the CLI, opens the view in a real tmux-hosted TUI
//! with `a`, asserts the list renders the seeded automation, and drives the
//! enable/disable toggle to confirm it round-trips through the store and
//! re-renders. Skips cleanly when tmux is unavailable.

use serial_test::serial;

use crate::harness::{require_tmux, TuiTestHarness};

#[test]
#[serial]
fn automations_view_lists_and_toggles_seeded_automation() {
    require_tmux!();

    let mut h = TuiTestHarness::new_in_tmp("automation_view_toggle");

    // Seed one automation via the CLI (no daemon). It creates no session, so
    // the TUI home still shows the empty-session state.
    let add = h.run_cli(&[
        "automation",
        "add",
        "--name",
        "nightly-digest",
        "--cron",
        "0 9 * * *",
        "--path",
        "/tmp",
        "--cmd",
        "bash",
        "--prompt",
        "echo hi",
        "--no-launch-daemon",
    ]);
    assert!(
        add.status.success(),
        "automation add failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr),
    );

    h.spawn_tui();
    h.wait_for("No sessions yet");

    // Open the Automations view.
    h.send_keys("a");
    h.wait_for("Automations");

    // The list shows the seeded automation and its cron, and the detail pane
    // shows it is enabled.
    h.assert_screen_contains("nightly-digest");
    h.assert_screen_contains("0 9 * * *");
    h.assert_screen_contains("enabled");

    // Toggle disable: Space flips enabled -> disabled, persists, and re-renders.
    h.send_keys("Space");
    h.wait_for("disabled");

    // Toggle back on.
    h.send_keys("Space");
    h.wait_for("enabled");

    // Esc returns to the session list.
    h.send_keys("Escape");
    h.wait_for("No sessions yet");
}

/// The add flow reuses the new-session wizard to collect a launch spec, then
/// the schedule dialog collects cron/prompt, and the result lands in the store
/// and re-renders in the view.
#[test]
#[serial]
fn add_flow_reaches_schedule_dialog_and_creates_automation() {
    require_tmux!();

    let mut h = TuiTestHarness::new_in_tmp("automation_view_add");
    h.spawn_tui();
    h.wait_for("No sessions yet");

    // Open the (empty) Automations view, then start an add.
    h.send_keys("a");
    h.wait_for("Automations");
    h.wait_for("No automations yet");
    h.send_keys("a");

    // The new-session wizard opens to collect the launch spec. Accept its
    // defaults (path = cwd, which exists; tool = the fake `claude` stub).
    h.wait_for("New Session");
    h.send_keys("Enter");

    // The schedule dialog collects the automation-only fields. Focus starts on
    // Name (the wizard does not pre-name automations); fill name, cron, prompt.
    h.wait_for("New Automation");
    h.type_text("nightly"); // name (focus starts here)
    h.send_keys("Tab"); // -> cron
    h.type_text("0 9 * * *");
    h.send_keys("Tab"); // -> prompt
    h.type_text("nightly report");
    h.send_keys("Enter");

    // Back in the view, the new automation (and its cron) is listed.
    h.wait_for("Automations");
    h.assert_screen_contains("nightly");
    h.assert_screen_contains("0 9 * * *");
}

/// Editing an existing automation round-trips through the store: the schedule
/// dialog opens prefilled, and submitting upserts and re-renders.
#[test]
#[serial]
fn edit_flow_round_trips_through_store() {
    require_tmux!();

    let mut h = TuiTestHarness::new_in_tmp("automation_view_edit");
    let add = h.run_cli(&[
        "automation",
        "add",
        "--name",
        "editme",
        "--cron",
        "0 9 * * *",
        "--path",
        "/tmp",
        "--cmd",
        "bash",
        "--prompt",
        "echo hi",
        "--no-launch-daemon",
    ]);
    assert!(
        add.status.success(),
        "automation add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    h.spawn_tui();
    h.wait_for("No sessions yet");
    h.send_keys("a");
    h.wait_for("Automations");
    h.assert_screen_contains("editme");

    // Open the edit dialog; it is prefilled with the existing cron.
    h.send_keys("e");
    h.wait_for("Edit Automation");
    h.assert_screen_contains("0 9 * * *");

    // Submit unchanged: upsert replaces by id and reopens the view.
    h.send_keys("Enter");
    h.wait_for("Automations");
    h.assert_screen_contains("editme");
}

/// Editing the launch spec detours through the new-session wizard (Ctrl+E from
/// the schedule dialog) and returns to the schedule dialog, preserving the
/// automation through the round-trip.
#[test]
#[serial]
fn edit_launch_spec_detours_through_wizard() {
    require_tmux!();

    let mut h = TuiTestHarness::new_in_tmp("automation_view_specedit");
    let add = h.run_cli(&[
        "automation",
        "add",
        "--name",
        "specedit",
        "--cron",
        "0 9 * * *",
        "--path",
        "/tmp",
        "--cmd",
        "bash",
        "--prompt",
        "echo hi",
        "--no-launch-daemon",
    ]);
    assert!(add.status.success());

    h.spawn_tui();
    h.wait_for("No sessions yet");
    h.send_keys("a");
    h.wait_for("Automations");

    // Open edit, then detour into the wizard to edit the launch spec.
    h.send_keys("e");
    h.wait_for("Edit Automation");
    h.send_keys("C-e");
    h.wait_for("New Session"); // wizard, prefilled from the spec
    h.send_keys("Enter"); // accept the spec unchanged

    // Back in the schedule dialog (prefilled), then submit.
    h.wait_for("Edit Automation");
    h.send_keys("Enter");
    h.wait_for("Automations");
    h.assert_screen_contains("specedit");
}
