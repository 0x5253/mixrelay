//! Instrument the production algorithm, not a model-only copy. No model atomics
//! or runtime dependencies enter the shared-memory ABI or real-time callback.

use loom::sync::{Arc, atomic as demand_atomics};
use loom::thread;

#[path = "../endpoint_transport/demand.rs"]
mod demand;
use demand::Demand;

#[test]
fn reader_observes_whole_callback_windows() {
    loom::model(|| {
        let demand = Arc::new(Demand::new());
        demand.reset(); // Lifecycle reset precedes all publishers/readers.
        demand.publish(32, 32);
        let publisher = Arc::clone(&demand);
        let writer = thread::spawn(move || publisher.publish(288, 256));
        if let Some(window) = demand.read() {
            assert!(matches!(window, (32, 32) | (288, 256)), "{window:?}");
        }
        writer.join().unwrap();
        assert_eq!(demand.read(), Some((288, 256)));
    });
}

#[test]
fn contending_publishers_never_mix_deadline_and_quantum() {
    loom::model(|| {
        let demand = Arc::new(Demand::new());
        demand.reset();
        let first = Arc::clone(&demand);
        let second = Arc::clone(&demand);
        let writer = thread::spawn(move || first.publish(32, 32));
        let other_writer = thread::spawn(move || second.publish(288, 256));
        if let Some(window) = demand.read() {
            assert!(
                matches!(window, (0, 0) | (32, 32) | (288, 256)),
                "{window:?}"
            );
        }
        writer.join().unwrap();
        other_writer.join().unwrap();
        // A contending publication may defer, but at least one must commit.
        assert!(matches!(demand.read(), Some((32, 32) | (288, 256))));
    });
}
