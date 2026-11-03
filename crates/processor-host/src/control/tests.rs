//! Active owner selection while a replacement is prepared for the same graph node.

use super::*;
use crate::{AudioLayout, shared_transport, worker::broker::consume_parameters};

#[test]
fn prepared_replacement_cannot_steal_controls_from_active_instance() {
    let registry = ProcessorControlRegistry::default();
    let node = NodeId::from("effect");
    let layout = AudioLayout {
        inputs: 2,
        outputs: 2,
        block_frames: 4,
    };
    let (_, active_mailbox, _, _) = shared_transport::memory::endpoints(
        layout,
        crate::instance_id::InstanceId::new(1).unwrap(),
    );
    let active = Arc::new(HostedProcessorControl::native(
        active_mailbox,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    active.active_owner.store(true, Ordering::Release);
    active
        .shared
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let _active_registration = registry.register(&node, &active);
    let (_, candidate_mailbox, _, _) = shared_transport::memory::endpoints(
        layout,
        crate::instance_id::InstanceId::new(2).unwrap(),
    );
    let candidate = Arc::new(HostedProcessorControl::native(
        candidate_mailbox,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    candidate
        .shared
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let _candidate_registration = registry.register(&node, &candidate);

    registry.set_parameter(&node, 7, 0.25).unwrap();
    let mut active_values = Vec::new();
    consume_parameters(active.shared.header(), |id, value| {
        active_values.push((id, value));
    });
    assert_eq!(active_values, [(7, 0.25)]);
    consume_parameters(candidate.shared.header(), |_, _| {
        panic!("candidate must remain untouched")
    });
    assert_eq!(
        registry
            .live_control(&node)
            .unwrap()
            .shared
            .header()
            .instance_id,
        active.shared.header().instance_id
    );

    candidate.active_owner.store(true, Ordering::Release);
    assert!(registry.set_parameter(&node, 7, 0.5).is_err());
    assert!(registry.live_control(&node).is_err());
    active.active_owner.store(false, Ordering::Release);
    registry.set_parameter(&node, 7, 0.75).unwrap();
    let mut candidate_values = Vec::new();
    consume_parameters(candidate.shared.header(), |id, value| {
        candidate_values.push((id, value));
    });
    assert_eq!(candidate_values, [(7, 0.75)]);
    consume_parameters(active.shared.header(), |_, _| {
        panic!("retired owner must remain untouched")
    });
}

#[test]
fn retry_cooldown_accepts_the_exact_boundary_on_the_failed_owners_clock() {
    let clock = Arc::new(mixrelay_time::ManualClock::default());
    let (_, mailbox, _, _) = shared_transport::memory::endpoints(
        AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    );
    let control = Arc::new(HostedProcessorControl::native(mailbox, clock.clone()));
    control.active_owner.store(true, Ordering::Release);
    control
        .shared
        .header()
        .worker_status
        .store(WORKER_FAILED, Ordering::Release);
    let registry = ProcessorControlRegistry::default();
    let node = NodeId::from("failed");
    let _registration = registry.register(&node, &control);
    let identity = control.shared.header().instance_id;
    let cooldown = Duration::from_mins(1);

    assert_eq!(
        registry.begin_failed_worker_retry(&node, identity, cooldown),
        Ok(())
    );
    clock.advance(cooldown.checked_sub(Duration::from_nanos(1)).unwrap());
    assert_eq!(
        registry.begin_failed_worker_retry(&node, identity, cooldown),
        Err("processor retry is cooling down")
    );
    clock.advance(Duration::from_nanos(1));
    assert_eq!(
        registry.begin_failed_worker_retry(&node, identity, cooldown),
        Ok(())
    );
    assert_eq!(
        registry.begin_failed_worker_retry(&node, identity, cooldown),
        Err("processor retry is cooling down")
    );
}
