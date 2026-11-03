use super::*;
#[test]
fn oldest_exited_role_logs_are_removed_while_live_and_unrecognized_files_survive() {
    let entries = [
        ("desktop-1.log", 1, true),
        ("desktop-2.log", 2, false),
        ("desktop-3.log", 3, false),
        ("desktop-4.log", 4, false),
        ("driver-runtime-5.log", 0, false),
        ("desktop-notes.log", 0, false),
        ("desktop-6.log.bak", 0, false),
    ]
    .into_iter()
    .map(|(name, modified, live)| LogEntry {
        name: name.into(),
        modified,
        live,
        token: name,
    });
    assert_eq!(obsolete_logs("desktop", 3, entries), ["desktop-2.log"]);
}
