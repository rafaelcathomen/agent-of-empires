use crate::harness::TuiTestHarness;
use serial_test::serial;

#[test]
#[serial]
fn automation_add_list_rm_round_trip() {
    let h = TuiTestHarness::new_in_tmp("automation_cli");

    // add
    let out = h.run_cli(&[
        "automation",
        "add",
        "--name",
        "slack digest",
        "--cron",
        "*/30 * * * *",
        "--path",
        "/tmp",
        "--tool",
        "claude",
        "--prompt",
        "summarize my slack",
        "--no-launch-daemon",
    ]);
    assert!(out.status.success(), "add failed: {:?}", out);

    // list shows it
    let list = h.run_cli(&["automation", "list"]);
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("slack digest"), "list was: {stdout}");

    // rm by short id (first token on the line)
    let id = stdout.split_whitespace().next().unwrap();
    let rm = h.run_cli(&["automation", "rm", id]);
    assert!(rm.status.success());
    let list2 = h.run_cli(&["automation", "list"]);
    assert!(!String::from_utf8_lossy(&list2.stdout).contains("slack digest"));
}
