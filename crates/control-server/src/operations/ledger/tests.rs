use super::*;

const TTL: Duration = Duration::from_mins(1);

fn at(seconds: u64) -> Instant {
    Instant::from_origin(Duration::from_secs(seconds))
}

fn ledger(capacity: usize) -> ReceiptLedger<&'static str, u8> {
    ReceiptLedger::new(capacity, TTL)
}

/// Issues `op-1`, `op-2`, … like a server-owned counter.
fn counter() -> impl FnMut() -> String {
    let mut next = 0;
    move || {
        next += 1;
        format!("op-{next}")
    }
}

fn unused() -> String {
    unreachable!("a client ID needs no generated identity")
}

#[test]
fn a_client_id_never_starts_work_twice() {
    let mut ledger = ledger(8);
    assert_eq!(
        ledger.admit(Some("client"), 1, at(0), unused),
        Ok("client".into())
    );
    assert_eq!(
        ledger.admit(Some("client"), 2, at(0), unused),
        Err(Rejection::Pending("client"))
    );
    assert_eq!(ledger.pending(), 1);
    assert_eq!(ledger.complete("client", "done", at(1)), Some(1));
    assert_eq!(
        ledger.admit(Some("client"), 3, at(2), unused),
        Err(Rejection::Complete("client", "done"))
    );
    assert_eq!(ledger.len(), 1);
}

#[test]
fn closed_admission_still_resolves_known_client_ids() {
    let mut ledger = ledger(8);
    ledger.admit(Some("client"), 1, at(0), unused).unwrap();
    ledger.complete("client", "done", at(0));
    ledger.close();
    assert_eq!(
        ledger.admit(Some("client"), 2, at(0), unused),
        Err(Rejection::Complete("client", "done"))
    );
    assert_eq!(
        ledger.admit(Some("other"), 2, at(0), unused),
        Err(Rejection::Full)
    );
    assert_eq!(
        ledger.admit(None, 2, at(0), counter()),
        Err(Rejection::Full)
    );
}

#[test]
fn capacity_counts_pending_and_unacknowledged_receipts_without_evicting_either() {
    let mut ledger = ledger(2);
    let mut issue = counter();
    let pending = ledger.admit(None, 1, at(0), &mut issue).unwrap();
    let done = ledger.admit(None, 2, at(0), &mut issue).unwrap();
    ledger.complete(&done, "done", at(0));
    assert_eq!(
        ledger.admit(None, 3, at(1), &mut issue),
        Err(Rejection::Full)
    );
    assert_eq!(ledger.snapshot(&pending, at(1)), Snapshot::Pending);
    assert_eq!(ledger.acknowledge(&done, at(1)), Ack::Completed);
    assert!(ledger.admit(None, 3, at(1), &mut issue).is_ok());
}

#[test]
fn generated_ids_skip_identities_in_use() {
    let mut ledger = ledger(8);
    ledger.admit(Some("op-1"), 1, at(0), unused).unwrap();
    assert_eq!(ledger.admit(None, 2, at(0), counter()), Ok("op-2".into()));
}

#[test]
fn completion_wakes_once_and_ignores_unknown_or_finished_receipts() {
    let mut ledger = ledger(8);
    ledger.admit(Some("client"), 7, at(0), unused).unwrap();
    assert_eq!(ledger.complete("missing", "done", at(0)), None);
    assert_eq!(ledger.complete("client", "first", at(0)), Some(7));
    assert_eq!(ledger.complete("client", "second", at(0)), None);
    assert_eq!(
        ledger.snapshot("client", at(0)),
        Snapshot::Complete("first")
    );
    assert_eq!(ledger.pending(), 0);
}

#[test]
fn only_completed_receipts_are_acknowledged() {
    let mut ledger = ledger(8);
    ledger.admit(Some("client"), 1, at(0), unused).unwrap();
    assert_eq!(ledger.acknowledge("client", at(0)), Ack::Pending);
    assert_eq!(ledger.acknowledge("missing", at(0)), Ack::Missing);
    ledger.complete("client", "done", at(0));
    assert_eq!(ledger.acknowledge("client", at(0)), Ack::Completed);
    assert_eq!(ledger.snapshot("client", at(0)), Snapshot::Missing);
    // An acknowledged ID can name new work.
    assert_eq!(
        ledger.admit(Some("client"), 2, at(0), unused),
        Ok("client".into())
    );
}

#[test]
fn completed_receipts_expire_exactly_at_their_ttl_and_pending_ones_never() {
    let mut ledger = ledger(8);
    ledger.admit(Some("done"), 1, at(0), unused).unwrap();
    ledger.admit(Some("running"), 2, at(0), unused).unwrap();
    ledger.complete("done", "result", at(10));
    assert_eq!(
        ledger.snapshot("done", at(69)),
        Snapshot::Complete("result")
    );
    assert_eq!(ledger.snapshot("done", at(70)), Snapshot::Missing);
    assert_eq!(ledger.snapshot("running", at(10_000)), Snapshot::Pending);
    assert_eq!(ledger.acknowledge("done", at(70)), Ack::Missing);
}

#[test]
fn an_expired_client_id_starts_new_work() {
    let mut ledger = ledger(1);
    ledger.admit(Some("client"), 1, at(0), unused).unwrap();
    ledger.complete("client", "old", at(0));
    assert_eq!(
        ledger.admit(Some("client"), 2, at(60), unused),
        Ok("client".into())
    );
    assert_eq!(ledger.snapshot("client", at(60)), Snapshot::Pending);
}

#[test]
fn taking_a_completed_response_frees_its_receipt() {
    let mut ledger = ledger(8);
    ledger.admit(Some("client"), 1, at(0), unused).unwrap();
    assert_eq!(ledger.take_completed("client"), None);
    assert_eq!(ledger.take_completed("missing"), None);
    ledger.complete("client", "done", at(0));
    assert_eq!(ledger.take_completed("client"), Some("done"));
    assert_eq!(ledger.len(), 0);
}
