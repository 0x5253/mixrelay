use super::*;

const LOADED: u64 = 0x5eed;

fn settled(endpoint: u8, sequence: u64, rate: u32, applied: bool) -> Configured {
    Configured {
        incarnation: LOADED,
        endpoint: [endpoint; 16],
        sequence,
        rate,
        formats: [1, 3],
        applied,
    }
}

fn replayed(entries: &[Configured]) -> Log {
    let mut log = Log::default();
    for entry in entries {
        log.record(*entry);
    }
    log.replayed(LOADED);
    log
}

#[test]
fn a_rejection_followed_by_other_settlements_is_still_attributed_to_its_request() {
    let mut log = replayed(&[]);
    let since = log.since().unwrap();
    // Reporting only the newest settlement per device lost this rejection.
    log.record(settled(1, 5, 96_000, false));
    log.record(settled(1, 6, 48_000, true));
    log.record(settled(2, 7, 96_000, true));
    assert_eq!(
        log.find([1; 16], since, Wanted::Rate(96_000)),
        Some(settled(1, 5, 96_000, false))
    );
}

#[test]
fn an_earlier_settlement_for_another_value_or_device_is_not_attributed() {
    let mut log = replayed(&[settled(1, 4, 96_000, false)]);
    let since = log.since().unwrap();
    log.record(settled(1, 5, 88_200, false));
    log.record(settled(2, 6, 96_000, false));
    assert_eq!(log.find([1; 16], since, Wanted::Rate(96_000)), None);
    log.record(settled(1, 7, 96_000, true));
    assert_eq!(
        log.find([1; 16], since, Wanted::Rate(96_000)),
        Some(settled(1, 7, 96_000, true))
    );
}

#[test]
fn a_replayed_old_rejection_cannot_bound_or_reject_a_new_request() {
    let mut log = Log::default();
    // A reconnect replays a rejection settled long before; the bound waits for the marker.
    log.record(settled(1, 9, 96_000, false));
    assert_eq!(log.since(), None);
    log.record(settled(1, 10, 48_000, true));
    log.replayed(LOADED);
    let since = log.since().unwrap();
    assert_eq!(since.sequence, 10);
    assert_eq!(log.find([1; 16], since, Wanted::Rate(96_000)), None);
}

#[test]
fn another_incarnation_never_answers_a_bound_and_offers_none_until_its_replay() {
    let mut log = replayed(&[settled(1, 50, 48_000, true)]);
    let since = log.since().unwrap();
    // coreaudiod restarted: sequences begin again at 1 under a new incarnation, and
    // its replayed settlements predate any request bounded by the old one.
    log.record(Configured {
        incarnation: LOADED + 1,
        ..settled(1, 1, 96_000, false)
    });
    assert_eq!(log.find([1; 16], since, Wanted::Rate(96_000)), None);
    assert_eq!(log.since(), None);
    log.replayed(LOADED + 1);
    assert_eq!(
        log.since(),
        Some(Since {
            incarnation: LOADED + 1,
            sequence: 1
        })
    );
}

#[test]
fn format_requests_match_their_direction_rate_and_sample_format() {
    let mut log = replayed(&[]);
    let since = log.since().unwrap();
    log.record(settled(1, 5, 48_000, false));
    let wanted = |render, rate, format| Wanted::Format {
        render,
        rate,
        format,
    };
    assert!(log.find([1; 16], since, wanted(true, 48_000, 3)).is_some());
    assert!(log.find([1; 16], since, wanted(false, 48_000, 1)).is_some());
    assert!(log.find([1; 16], since, wanted(true, 48_000, 1)).is_none());
    assert!(log.find([1; 16], since, wanted(true, 96_000, 3)).is_none());
}

#[test]
fn settlements_for_many_endpoints_over_time_keep_arriving() {
    let mut log = replayed(&[]);
    // A per-endpoint map capped at 64 entries silently dropped every later device.
    for sequence in 1..1_000_u64 {
        let endpoint = u8::try_from(sequence % 100).unwrap();
        log.record(settled(endpoint, sequence, 96_000, true));
    }
    let since = log.since().unwrap();
    log.record(settled(0, 1_000, 96_000, false));
    assert_eq!(
        log.find([0; 16], since, Wanted::Rate(96_000)),
        Some(settled(0, 1_000, 96_000, false))
    );
    assert_eq!(log.entries.len(), RETAINED);
}

#[test]
fn settlements_resent_after_a_reconnect_are_kept_once() {
    let mut log = Log::default();
    for sequence in [5, 6, 5, 6, 7] {
        log.record(settled(1, sequence, 48_000, true));
    }
    assert_eq!(
        log.entries
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        [5, 6, 7]
    );
}
