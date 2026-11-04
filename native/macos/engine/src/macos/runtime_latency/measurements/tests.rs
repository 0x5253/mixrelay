use super::{Measurements, arrival_failure};
use crate::macos::runtime_latency::fixture::{
    canonical_snapshot_route, feedback_plan, observation,
};
use crate::macos_devices::latency::Observation;
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn canonical_snapshot_route_publishes_measured_source_plus_prepared_quantum() {
    use mixrelay_audio_io::residence::ResidenceSnapshot;
    use mixrelay_render::BlockProcessor;
    let composed = canonical_snapshot_route();
    let mut processor = BlockProcessor::prepare_with_hosted_processors(
        &composed.execution,
        Arc::new(mixrelay_render::PeakMeterBank::new(32, 64)),
        &crate::macos_devices::latency::tests::Factory,
    )
    .unwrap();
    let plan = Arc::clone(processor.plan());
    assert_eq!(plan.source_bindings().len(), 1);
    assert_eq!(
        plan.source_bindings()[0].virtual_endpoint_id(),
        Some(&"A".into())
    );
    assert_eq!(plan.sink_bindings().len(), 1);
    assert_eq!(
        plan.sink_bindings()[0].virtual_endpoint_id(),
        Some(&"D".into())
    );
    let mut d_pcm = [0.0; 64];
    for expected in [0.0, 0.25] {
        processor
            .process_block(32, &[&[0.25; 64]], &mut [&mut d_pcm])
            .unwrap();
        assert_eq!(d_pcm.map(f32::to_bits), [expected; 64].map(f32::to_bits));
    }
    assert!(processor.acknowledge_content_latency());
    let a = composed.native_devices[&"A".into()].as_str();
    let d = composed.native_devices[&"D".into()].as_str();
    let rates = BTreeMap::from([("D".into(), 48_000)]);
    let mut source = observation();
    let mut destination = observation();
    destination.identity += 1;
    let current =
        |source, destination| BTreeMap::from([(a.into(), source), (d.into(), destination)]);
    let mut measurement = Measurements::default();
    assert_eq!(
        measurement.observe(&plan, 1, current(source, destination), &rates)[&"D".into()],
        None
    );
    let ticks = mixrelay_realtime::Timebase::current().ticks(std::time::Duration::from_millis(1));
    source.native.acquisition = ResidenceSnapshot {
        sum: ticks,
        weight: 1,
        invalid: 0,
    };
    destination.native.presentation = source.native.acquisition;
    let frames =
        measurement.observe(&plan, 1, current(source, destination), &rates)[&"D".into()].unwrap();
    assert!(
        (128..=129).contains(&frames),
        "2 ms plus 32 graph frames: {frames}"
    );
    // Source disturbances must still reach the destination through its bus.
    source.faults += 1;
    assert_eq!(
        measurement.observe(&plan, 1, current(source, destination), &rates)[&"D".into()],
        Some(frames)
    );
    source.epoch += 1;
    assert_eq!(
        measurement.observe(&plan, 1, current(source, destination), &rates)[&"D".into()],
        None
    );
}

#[test]
fn contended_or_missing_taps_skip_windows_but_real_clock_changes_reset() {
    let plan = feedback_plan();
    let id = plan.sink_bindings()[0]
        .virtual_endpoint_id()
        .unwrap()
        .clone();
    let rates = BTreeMap::from([(id.clone(), 48_000)]);
    let mut measurement = Measurements::default();
    let mut value = observation();
    let current = |value| BTreeMap::from([("virtual".into(), value)]);
    assert_eq!(
        measurement.observe(&plan, 1, current(value), &rates)[&id],
        None
    );
    value.native.acquisition.weight += 1;
    value.native.presentation.weight += 1;
    assert_eq!(
        measurement.observe(&plan, 1, current(value), &rates)[&id],
        Some(0)
    );
    let mut busy = value;
    busy.sampled = false;
    assert_eq!(
        measurement.observe(&plan, 1, current(busy), &rates)[&id],
        Some(0)
    );
    assert_eq!(
        measurement.observe(&plan, 1, BTreeMap::new(), &rates)[&id],
        Some(0)
    );
    value.native.acquisition.weight += 1;
    value.native.presentation.weight += 1;
    assert_eq!(
        measurement.observe(&plan, 1, current(value), &rates)[&id],
        Some(0)
    );
    for change in 0..4 {
        let mut changed = value;
        changed.sampled = false;
        match change {
            0 => changed.native_rate = 44_100,
            1 => changed.epoch += 1,
            2 => changed.generation += 1,
            _ => changed.identity += 1,
        }
        assert_eq!(
            measurement.observe(&plan, 1, current(changed), &rates)[&id],
            None
        );
    }
}

#[test]
fn unusable_windows_name_their_first_reason_once() {
    let plan = feedback_plan();
    let id = plan.sink_bindings()[0]
        .virtual_endpoint_id()
        .unwrap()
        .clone();
    let rates = BTreeMap::from([(id.clone(), 48_000)]);
    let mut measurement = Measurements::default();
    let mut value = observation();
    let current = |value| BTreeMap::from([("virtual".into(), value)]);
    let reason = |measurement: &Measurements| measurement.endpoints[&id].diagnosis.clone();
    measurement.observe(&plan, 1, BTreeMap::new(), &rates);
    assert_eq!(
        reason(&measurement).as_deref(),
        Some("virtual: no observation for the active plan")
    );
    measurement.observe(&plan, 1, current(value), &rates);
    assert_eq!(
        reason(&measurement).as_deref(),
        Some("device 1: first window")
    );
    value.native.acquisition.weight += 1;
    value.native.presentation.weight += 1;
    assert_eq!(
        measurement.observe(&plan, 1, current(value), &rates)[&id],
        Some(0)
    );
    assert_eq!(reason(&measurement), None);
    value.faults += 1;
    measurement.observe(&plan, 1, current(value), &rates);
    assert_eq!(
        reason(&measurement).as_deref(),
        Some("device 1: binding faults advanced")
    );
    value.native_rate = 44_100;
    value.native.acquisition.weight += 1;
    value.native.presentation.weight += 1;
    measurement.observe(&plan, 1, current(value), &rates);
    assert_eq!(
        reason(&measurement).as_deref(),
        Some("device 1: clock context changed")
    );
    // An unsampled tap is named before the fault it also carries.
    let mut both = value;
    both.sampled = false;
    both.faults += 1;
    measurement.observe(&plan, 1, current(both), &rates);
    assert_eq!(
        reason(&measurement).as_deref(),
        Some("virtual: taps not sampled")
    );
}

#[test]
fn arrival_contention_disturbs_while_an_invalid_request_is_unknown() {
    assert_eq!(
        arrival_failure(mixrelay_render::ArrivalError::Contended),
        (true, "arrival read contended".into())
    );
    assert_eq!(
        arrival_failure(mixrelay_render::ArrivalError::Invalid("size".into())),
        (false, "arrival invalid: size".into())
    );
}

#[test]
fn reporter_windows_publish_real_direct_delay_and_reset_graph_and_lease() {
    use mixrelay_audio_io::residence::ResidenceSnapshot;
    let plan = feedback_plan();
    let id = plan.sink_bindings()[0]
        .virtual_endpoint_id()
        .unwrap()
        .clone();
    let rates = BTreeMap::from([(id.clone(), 48_000)]);
    let mut value = observation();
    let current = |value| BTreeMap::from([("virtual".into(), value)]);
    let mut measurement = Measurements::default();
    assert_eq!(
        measurement.observe(&plan, 1, current(value), &rates)[&id],
        None
    );
    let ticks = mixrelay_realtime::Timebase::current().ticks(std::time::Duration::from_millis(1));
    value.native.acquisition = ResidenceSnapshot {
        sum: ticks,
        weight: 1,
        invalid: 0,
    };
    value.native.presentation = value.native.acquisition;
    let first = measurement.observe(&plan, 1, current(value), &rates)[&id].unwrap();
    assert!((96..=97).contains(&first));
    value.faults += 1;
    assert_eq!(
        measurement.observe(&plan, 1, current(value), &rates)[&id],
        Some(first)
    );
    assert_eq!(
        measurement.observe(&plan, 2, current(value), &rates)[&id],
        None
    );
    // A freshly claimed lease has fresh window baselines and no retained values.
    measurement = Measurements::default();
    assert_eq!(
        measurement.observe(&plan, 2, current(value), &rates)[&id],
        None
    );
    value.native.acquisition.invalid += 1;
    assert_eq!(
        measurement.observe(&plan, 2, current(value), &rates)[&id],
        None
    );
}

#[test]
fn residence_invalid_windows_retain_published_latency_and_then_recover() {
    use mixrelay_audio_io::residence::{Passage, Residence};
    let plan = feedback_plan();
    let id = plan.sink_bindings()[0]
        .virtual_endpoint_id()
        .unwrap()
        .clone();
    let rates = BTreeMap::from([(id.clone(), 48_000)]);
    // Just under 32 native frames at 48 kHz, so the planned 32 round up to 64 once.
    let ticks = mixrelay_realtime::Timebase::current().ticks(std::time::Duration::from_micros(666));
    for index in 0..4 {
        let mut measurement = Measurements::default();
        let mut value = observation();
        value.direct = false;
        let current = |value| BTreeMap::from([("virtual".into(), value)]);
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            None
        );
        let advance = |value: &mut Observation| {
            for tap in [
                &mut value.native.acquisition,
                &mut value.native.presentation,
                &mut value.src_input,
                &mut value.src_output,
            ] {
                tap.weight += 32;
            }
            for queue in [
                &mut value.native.native_queue,
                &mut value.native.graph_queue,
            ] {
                queue.producer.position += 32;
                queue.consumer.position += 32;
            }
            value.native.native_queue.consumer.area += 32 * ticks;
        };
        advance(&mut value);
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            Some(64)
        );
        // A FIFO consumer passes frames it never read, or an SRC sample is invalid.
        // No shortage/fault counter changes in this window.
        if index < 2 {
            let passage = Passage::default();
            passage.consumer.advance(32, 0, 0);
            let discarded = passage.consumer.snapshot().unwrap().discarded;
            let queue = if index == 0 {
                &mut value.native.native_queue
            } else {
                &mut value.native.graph_queue
            };
            queue.consumer.discarded += discarded;
        } else {
            let residence = Residence::default();
            residence.record(None, 32);
            let invalid = residence.snapshot().unwrap().invalid;
            let tap = if index == 2 {
                &mut value.src_input
            } else {
                &mut value.src_output
            };
            tap.invalid += invalid;
        }
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            Some(64)
        );
        advance(&mut value);
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            Some(64)
        );
    }
}
