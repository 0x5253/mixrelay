use std::{
    fs::OpenOptions,
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use mixrelay_test_support::FixtureDir;

use super::*;
use crate::{STATE_CAPACITY, WORKER_READY};

fn layout() -> AudioLayout {
    AudioLayout {
        inputs: 2,
        outputs: 2,
        block_frames: 4,
    }
}

fn mapping_length(layout: AudioLayout) -> u64 {
    let samples = (layout.inputs + layout.outputs) * layout.block_frames;
    u64::try_from(size_of::<SharedHeader>() + CHUNK_CAPACITY + samples * size_of::<f32>()).unwrap()
}

#[test]
fn worker_lease_survives_either_endpoint_and_releases_after_both() {
    let directory = FixtureDir::new("processor-lease").unwrap();
    let path = directory.join("audio");
    let _parent = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (audio, control) = worker(&path).unwrap();
    assert!(worker(&path).is_err());
    drop(audio);
    assert!(worker(&path).is_err());
    drop(control);
    let (audio, control) = worker(&path).unwrap();
    drop(control);
    assert!(worker(&path).is_err());
    drop(audio);
    assert!(worker(&path).is_ok());
}

#[test]
fn failed_admission_releases_worker_lease() {
    let directory = FixtureDir::new("processor-invalid-mapping").unwrap();
    let path = directory.join("audio");
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.set_len(8).unwrap();
    assert!(worker(&path).is_err());
    assert!(file.try_lock().is_ok());
    file.unlock().unwrap();
    file.set_len(0).unwrap();
    assert!(worker(&path).is_err());
    assert!(file.try_lock().is_ok());
}

#[test]
fn creation_rejects_overflow_without_creating_a_file_or_replacing_an_owner() {
    let directory = FixtureDir::new("processor-create-bounds").unwrap();
    let path = directory.join("audio");
    assert!(
        create(
            &path,
            AudioLayout {
                inputs: usize::MAX,
                ..layout()
            },
            crate::instance_id::InstanceId::new(1).unwrap()
        )
        .is_err()
    );
    assert!(!path.exists());
    let _owner = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    assert!(
        create(
            &path,
            layout(),
            crate::instance_id::InstanceId::new(1).unwrap()
        )
        .is_err()
    );
    assert!(worker(&path).is_ok());
}

#[test]
fn responses_need_no_additional_transport_file() {
    let directory = FixtureDir::new("processor-response-files").unwrap();
    let path = directory.join("audio");
    let _transport = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn full_admitted_state_round_trips_without_growing_mapping() {
    let directory = FixtureDir::new("processor-state-chunk-size").unwrap();
    let path = directory.join("audio");
    let layout = AudioLayout {
        inputs: 2,
        outputs: 2,
        block_frames: 128,
    };
    let (audio, control) = create(
        &path,
        layout,
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_, worker) = worker(&path).unwrap();
    let mapping_size = mapping_length(layout);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), mapping_size);
    control
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let child = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.pending_request().is_none() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        let request = worker.pending_request().unwrap().0;
        worker.publish(request, Ok(vec![0xa5; STATE_CAPACITY]));
    });
    assert_eq!(
        control
            .try_lock()
            .unwrap()
            .request_blob(0, Duration::from_secs(2))
            .unwrap(),
        vec![0xa5; STATE_CAPACITY]
    );
    child.join().unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), mapping_size);
    assert_eq!(
        control.header().state_length.load(Ordering::Acquire),
        u32::try_from(STATE_CAPACITY).unwrap()
    );
    drop(audio);
    drop(control);
}

#[test]
fn thirty_two_two_channel_128_frame_mappings_do_not_reserve_state_capacity() {
    let directory = FixtureDir::new("processor-mapping-total").unwrap();
    let layout = AudioLayout {
        inputs: 2,
        outputs: 2,
        block_frames: 128,
    };
    let mut transports = Vec::new();
    let mut total = 0_u64;
    for index in 0..32 {
        let path = directory.join(format!("audio-{index}"));
        let (audio, control) = create(
            &path,
            layout,
            crate::instance_id::InstanceId::new(1).unwrap(),
        )
        .unwrap();
        total += std::fs::metadata(&path).unwrap().len();
        transports.push((audio, control));
    }
    let per_mapping = mapping_length(layout);
    assert_eq!(total, 32 * per_mapping);
    const { assert!(CHUNK_CAPACITY < STATE_CAPACITY) };
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 32);
}

#[test]
fn state_snapshots_and_pcm_exchange_use_disjoint_regions_concurrently() {
    let directory = FixtureDir::new("processor-concurrent-regions").unwrap();
    let path = directory.join("audio");
    let (mut audio, control) = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (mut worker_audio, worker_control) = worker(&path).unwrap();
    control
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let child = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut samples = [0.0; 8];
        while !worker_finished.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            if worker_audio
                .receive(|input| samples[..input.len()].copy_from_slice(input))
                .is_some()
            {
                assert!(worker_audio.publish(|output| output.copy_from_slice(&samples)));
            }
            if let Some((request, _)) = worker_control.pending_request() {
                worker_control.publish(request, Ok(vec![0xa5; 4096]));
            }
            thread::yield_now();
        }
    });
    let snapshots = thread::spawn(move || {
        for _ in 0..64 {
            assert_eq!(
                control
                    .try_lock()
                    .unwrap()
                    .request_blob(0, Duration::from_secs(2))
                    .unwrap(),
                vec![0xa5; 4096]
            );
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    for sequence in 1..=1000 {
        let sample = f32::from(u16::try_from(sequence).unwrap());
        assert!(audio.submit(sequence, &[sample; 8], 4, || {}));
        let mut output = [0.0; 8];
        while !audio.receive(&mut output, 4, |_| {}) {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert_eq!(output.map(f32::to_bits), [sample; 8].map(f32::to_bits));
    }
    snapshots.join().unwrap();
    finished.store(true, Ordering::Release);
    child.join().unwrap();
}

#[test]
fn zero_identity_rejects_worker_admission_and_releases_its_file_lease() {
    use std::io::{Seek, SeekFrom, Write};
    let directory = FixtureDir::new("processor-zero-id").unwrap();
    let path = directory.join("audio");
    let endpoints = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(7).unwrap(),
    )
    .unwrap();
    drop(endpoints);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::Start(
        std::mem::offset_of!(SharedHeader, instance_id) as u64,
    ))
    .unwrap();
    file.write_all(&0_u64.to_ne_bytes()).unwrap();
    assert!(worker(&path).is_err());
    assert!(file.try_lock().is_ok());
    file.unlock().unwrap();
}
