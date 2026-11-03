#![cfg(feature = "native-lab")]
use super::super::super::applications::tests::ObservedDriver;
use super::*;

fn transfer(next: &mut EndpointSet, active: &mut EndpointSet) {
    let mut valid = false;
    let measured = allocation_counter::measure(|| {
        valid = next.can_retain_from(active);
        if valid {
            next.retain_from(active);
        }
    });
    assert!(valid);
    assert_eq!(measured.count_total, 0);
    assert_eq!(
        measured.count_current, 0,
        "RT transfer must not reclaim owners"
    );
}

fn silence_bytes(set: &EndpointSet) -> usize {
    set.nodes
        .values()
        .filter_map(|slot| slot.owner.as_ref())
        .map(|owner| std::mem::size_of_val(owner.silence.as_ref()))
        .sum()
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one native ownership scenario verifies transfer, rollback and final retirement together"
)]
fn native_endpoint_transfer_preserves_owners_and_bounds_rt_storage() {
    let _driver_devices = DriverFixture::new();
    let _driver = ObservedDriver::new();
    let format = render_format();
    let one = specs(vec![("a".into(), device_spec(A))]).unwrap();
    let two = specs(vec![
        ("a".into(), device_spec(A)),
        ("b".into(), device_spec(B)),
    ])
    .unwrap();
    let mut first = EndpointSet::prepare(
        &mixrelay_pipewire::StreamOwners::default(),
        &one,
        &EndpointSpecs::new(),
        format,
        format,
    )
    .unwrap();
    transfer(&mut first, &mut EndpointSet::default());
    let serials = inventory();
    assert_eq!(serials.len(), 4);
    assert_eq!(silence_bytes(&first), 32 * 128 * 4);
    let mut added = EndpointSet::prepare(
        &mixrelay_pipewire::StreamOwners::default(),
        &two,
        &one,
        format,
        format,
    )
    .unwrap();
    assert_eq!(inventory().len(), 4);
    assert!(!added.can_retain_from(&EndpointSet::default()));
    transfer(&mut added, &mut first);
    drop(first); // Off RT: the retained envelope owns no streams or scratch now.
    assert_eq!(silence_bytes(&added), 2 * 32 * 128 * 4);
    assert!(
        serials
            .iter()
            .all(|(name, serial)| inventory().get(name) == Some(serial))
    );

    let mut invalid = device_spec("invalid");
    invalid.capture.as_mut().unwrap().channels = 65;
    let before_failure = inventory();
    let invalid = specs(vec![
        ("new".into(), device_spec("new")),
        ("z-invalid".into(), invalid),
    ])
    .unwrap();
    assert!(
        EndpointSet::prepare(
            &mixrelay_pipewire::StreamOwners::default(),
            &invalid,
            &two,
            format,
            format
        )
        .is_err()
    );
    assert_eq!(
        inventory(),
        before_failure,
        "failed preparation must unwind only new owners"
    );

    let mut reduced = EndpointSet::prepare(
        &mixrelay_pipewire::StreamOwners::default(),
        &one,
        &two,
        format,
        format,
    )
    .unwrap();
    transfer(&mut reduced, &mut added);
    drop(added);
    assert_eq!(inventory(), serials);
    let mut result = Ok(ProcessReport::default());
    let measured = allocation_counter::measure(|| {
        result = reduced.finish_block(128);
    });
    result.unwrap();
    assert_eq!(measured.count_total, 0);
    assert_eq!(measured.count_current, 0);
    assert!(reduced.finish_block(129).is_err());
    drop(reduced);
    assert_eq!(
        inventory(),
        serials,
        "retiring graph clients must preserve Driver devices"
    );
    println!(
        "single_duplex_silence_bytes=16384; two_duplex_silence_bytes=32768; transfer_allocations=0; transfer_deallocations=0; rollback=passed; driver_nodes_after_retirement=4"
    );
}
