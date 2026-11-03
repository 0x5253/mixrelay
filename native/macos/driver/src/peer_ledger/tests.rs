use super::*;

impl PeerIdentity for usize {
    fn identity(&self) -> usize {
        *self
    }
}

fn peer(object: usize, log: Log) -> Peer<usize> {
    Peer {
        object,
        subscribed: true,
        settled: None,
        log,
        incompatible: false,
    }
}

fn settled(incarnation: u64, sequence: u64, applied: bool) -> Configured {
    Configured {
        incarnation,
        endpoint: [1; 16],
        sequence,
        rate: 96_000,
        formats: [1, 3],
        applied,
    }
}

#[test]
fn a_stale_connection_neither_bounds_nor_answers_requests_of_a_new_hal_load() {
    let mut old = Log::default();
    old.record(settled(1, 5, true));
    old.replayed(1);
    // A new HAL load replays a rejection of the same value settled before any request.
    let mut new = Log::default();
    new.record(settled(2, 3, false));
    let mut peers = Peers::default();
    peers.values.push(peer(1, old));
    peers.values.push(peer(2, new));
    let since = peers.since().unwrap();
    assert_eq!(since.incarnation, 1);
    assert_eq!(peers.settled([1; 16], since, Wanted::Rate(96_000)), None);
    peers.values[1].log.replayed(2);
    let current = peers.values[1].object;
    peers.retire_before(current);
    assert_eq!(peers.values.len(), 1);
    let since = peers.since().unwrap();
    assert_eq!((since.incarnation, since.sequence), (2, 3));
    assert_eq!(peers.settled([1; 16], since, Wanted::Rate(96_000)), None);
    peers.values[0].log.record(settled(2, 4, false));
    assert_eq!(
        peers.settled([1; 16], since, Wanted::Rate(96_000)),
        Some(settled(2, 4, false))
    );
}

#[test]
fn settlement_requires_the_current_publication_and_incompatible_reports_do_not_apply() {
    let mut peer = peer(1, Log::default());
    peer.receive(
        Some(3),
        Ok(Report::Inventory {
            generation: 2,
            applied: true,
        }),
    );
    assert_eq!(peer.settled, None);
    peer.receive(
        None,
        Ok(Report::Inventory {
            generation: 3,
            applied: true,
        }),
    );
    assert_eq!(peer.settled, None);
    peer.receive(
        Some(3),
        Ok(Report::Inventory {
            generation: 3,
            applied: false,
        }),
    );
    assert_eq!(peer.settled, Some((3, Settlement::Rejected)));
    peer.receive(Some(3), Err(Incompatible));
    assert!(peer.incompatible);
    assert_eq!(peer.settled, Some((3, Settlement::Rejected)));
}
