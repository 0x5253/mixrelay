use super::{StopMode, Worker};
use crate::recording_files::memory::MemoryFiles;
use crate::{
    Encoding, RecorderAction, RecorderRequest, RecorderState, RecorderStatus, RecorderTap,
    transport::{PACKET_FRAMES, PACKETS, Transport},
};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;

#[test]
fn single_rf64_take_keeps_original_path_and_total_frames_across_pause_resume_and_stop() {
    let directory = Path::new("/recorder-rf64-lifecycle");
    let files = MemoryFiles::default();
    let path = directory.join("first.wav").to_str().unwrap().to_owned();
    let transport = Transport::new(2, std::hint::spin_loop).unwrap();
    let tap = RecorderTap::new(Arc::clone(&transport));
    let status = Arc::new(Mutex::new(RecorderStatus::default()));
    let mut worker = Worker::new(transport, Arc::clone(&status), files.clone()).unwrap();
    let request = RecorderRequest {
        action: RecorderAction::Record,
        path: Some(path.clone()),
        sample_rate: None,
        encoding: Some(Encoding::Float32),
    };
    tap.capture(&[], 48_000);
    worker.record(&request).unwrap();
    tap.capture(&[0.25; 6], 48_000);
    worker.stop(StopMode::Pause).unwrap();
    let paused = status.lock().clone();
    assert_eq!(
        (paused.state, paused.frames, paused.path.as_deref()),
        (RecorderState::Paused, 3, Some(path.as_str()))
    );
    worker.record(&request).unwrap();
    tap.capture(&[0.5; 2], 48_000);
    worker.stop(StopMode::Finish).unwrap();
    let stopped = status.lock().clone();
    assert_eq!(
        (stopped.state, stopped.frames, stopped.path.as_deref()),
        (RecorderState::Stopped, 4, Some(path.as_str()))
    );
    let mut second = files.reader(&path);
    assert_eq!(
        second
            .samples::<f32>()
            .map(|sample| sample.unwrap().to_bits())
            .collect::<Vec<_>>(),
        [0.25_f32, 0.25, 0.25, 0.25, 0.25, 0.25, 0.5, 0.5].map(f32::to_bits)
    );
    assert_eq!(files.len(), 1);
}

#[test]
fn a_half_second_writer_stall_preserves_paced_quantum_pcm_in_fixed_memory() {
    let directory = Path::new("/recorder-stalled-writer");
    let files = MemoryFiles::default();
    let path = directory.join("packed.wav");
    let transport = Transport::new(8, std::hint::spin_loop).unwrap();
    let tap = RecorderTap::new(Arc::clone(&transport));
    let status = Arc::new(Mutex::new(RecorderStatus::default()));
    let mut worker = Worker::new(transport, Arc::clone(&status), files.clone()).unwrap();
    tap.capture(&[], 48_000);
    worker
        .record(&RecorderRequest {
            action: RecorderAction::Record,
            path: Some(path.to_str().unwrap().into()),
            sample_rate: Some(96_000),
            encoding: Some(Encoding::Pcm24),
        })
        .unwrap();
    // Deliberately do not service the worker for 750 consecutive 32-frame
    // callbacks: 500 ms of input at 48 kHz. No wall-clock/timer assumption.
    let allocation = allocation_counter::measure(|| {
        for _ in 0..750 {
            tap.capture(&[0.25; 32 * 8], 48_000);
        }
    });
    assert_eq!((allocation.count_total, allocation.count_current), (0, 0));
    worker.stop(StopMode::Finish).unwrap();
    let snapshot = status.lock().clone();
    assert_eq!(
        (snapshot.state, snapshot.frames),
        (RecorderState::Stopped, 48_000)
    );
    let mut reader = files.reader(path);
    assert_eq!(
        (
            reader.duration(),
            reader.spec().channels,
            reader.spec().sample_rate
        ),
        (48_000, 8, 96_000)
    );
    assert!(reader.samples::<i32>().all(|sample| sample.unwrap() != 0));
}

#[test]
fn admission_error_finalizes_a_valid_unpublished_partial_packet() {
    let directory = Path::new("/recorder-partial-prefix");
    let files = MemoryFiles::default();
    let path = directory.join("prefix.wav");
    let transport = Transport::new(2, std::hint::spin_loop).unwrap();
    let tap = RecorderTap::new(Arc::clone(&transport));
    let status = Arc::new(Mutex::new(RecorderStatus::default()));
    let mut worker = Worker::new(transport, Arc::clone(&status), files.clone()).unwrap();
    tap.capture(&[], 48_000);
    worker
        .record(&RecorderRequest {
            action: RecorderAction::Record,
            path: Some(path.to_str().unwrap().into()),
            sample_rate: None,
            encoding: Some(Encoding::Float32),
        })
        .unwrap();
    tap.capture(&[0.25; 64], 48_000);
    worker.drain().unwrap();
    assert_eq!(status.lock().frames, 0);
    tap.capture(&[0.5], 48_000);
    assert!(
        worker
            .stop(StopMode::Finish)
            .unwrap_err()
            .to_string()
            .contains("invalid PCM")
    );
    let snapshot = status.lock().clone();
    assert_eq!(
        (snapshot.state, snapshot.frames),
        (RecorderState::Failed, 32)
    );
    let mut reader = files.reader(path);
    assert_eq!(reader.duration(), 32);
    assert!(
        reader
            .samples::<f32>()
            .all(|sample| sample.unwrap().to_bits() == 0.25_f32.to_bits())
    );
}

#[test]
fn admission_overrun_finalizes_every_previously_accepted_packet_as_a_failed_take() {
    let directory = Path::new("/recorder-overrun-prefix");
    let files = MemoryFiles::default();
    let path = directory.join("prefix.wav");
    let transport = Transport::new(1, std::hint::spin_loop).unwrap();
    let tap = RecorderTap::new(Arc::clone(&transport));
    let status = Arc::new(Mutex::new(RecorderStatus::default()));
    let mut worker = Worker::new(transport, Arc::clone(&status), files.clone()).unwrap();
    tap.capture(&[], 48_000);
    worker
        .record(&RecorderRequest {
            action: RecorderAction::Record,
            path: Some(path.to_str().unwrap().into()),
            sample_rate: Some(48_000),
            encoding: Some(Encoding::Float32),
        })
        .unwrap();
    for _ in 0..=PACKETS {
        tap.capture(&[0.5; PACKET_FRAMES], 48_000);
    }
    assert!(worker.stop(StopMode::Finish).is_err());
    assert_eq!(status.lock().state, RecorderState::Failed);
    assert_eq!(
        status.lock().frames,
        u64::try_from(PACKETS * PACKET_FRAMES).unwrap()
    );
    let mut reader = files.reader(path);
    assert_eq!(
        usize::try_from(reader.duration()).unwrap(),
        PACKETS * PACKET_FRAMES
    );
    assert!(
        reader
            .samples::<f32>()
            .all(|sample| sample.unwrap().to_bits() == 0.5_f32.to_bits())
    );

    let missing_path = directory.join("missing/new.wav");
    files.reject_create(&missing_path, std::io::ErrorKind::NotFound);
    let rejected = worker.command(&RecorderRequest {
        action: RecorderAction::Record,
        path: Some(missing_path.to_str().unwrap().into()),
        sample_rate: None,
        encoding: None,
    });
    assert!(rejected.unwrap_err().to_string().contains("cannot create"));
    assert!(
        status
            .lock()
            .error
            .as_ref()
            .unwrap()
            .contains("cannot create")
    );
    assert!(
        worker
            .command(&RecorderRequest {
                action: RecorderAction::Stop,
                path: None,
                sample_rate: None,
                encoding: None
            })
            .unwrap_err()
            .to_string()
            .contains("cannot create")
    );
}

#[test]
fn recording_preserves_all_frames_across_pause_resume_and_rate_changes() {
    for encoding in [Encoding::Pcm16, Encoding::Pcm24, Encoding::Float32] {
        let directory = Path::new("/recorder-rate-lifecycle");
        let files = MemoryFiles::default();
        let path = directory.join("take.wav");
        let transport = Transport::new(2, std::hint::spin_loop).unwrap();
        let tap = RecorderTap::new(Arc::clone(&transport));
        let status = Arc::new(Mutex::new(RecorderStatus::default()));
        let mut worker = Worker::new(transport, Arc::clone(&status), files.clone()).unwrap();
        tap.capture(&[], 48_000);
        let request = RecorderRequest {
            action: RecorderAction::Record,
            path: Some(path.to_str().unwrap().into()),
            sample_rate: Some(48_000),
            encoding: Some(encoding),
        };
        worker.record(&request).unwrap();
        for _ in 0..100 {
            tap.capture(&[0.25; 480], 48_000);
            worker.drain().unwrap();
        }
        worker.stop(StopMode::Pause).unwrap();
        let paused = status.lock().clone();
        assert_eq!(
            (paused.state, paused.frames),
            (RecorderState::Paused, 24_000)
        );
        // Paused callbacks must not extend the take.
        for _ in 0..100 {
            tap.capture(&[0.25; 480], 48_000);
        }
        worker.record(&request).unwrap();
        for _ in 0..102 {
            tap.capture(&[0.25; 960], 96_000);
            worker.drain().unwrap();
        }
        assert_eq!(status.lock().state, RecorderState::Recording);
        worker.stop(StopMode::Finish).unwrap();
        let finished = status.lock().clone();
        assert_eq!(
            (finished.state, finished.frames, finished.error),
            (RecorderState::Stopped, 48_480, None)
        );
        assert_eq!(
            mixrelay_test_support::rf64::inspect_bytes(&files.bytes(&path))
                .unwrap()
                .frames,
            48_480
        );
        assert_eq!(files.len(), 1);
    }
}

#[test]
fn control_owner_disconnect_finalizes_the_accepted_prefix_in_every_format() {
    for channels in 1..=mixrelay_audio_io::formats::MAX_RECORDING_CHANNELS {
        for encoding in [Encoding::Pcm16, Encoding::Pcm24, Encoding::Float32] {
            let directory = Path::new("/rec-disconnect");
            let files = MemoryFiles::default();
            let path = directory.join("prefix.wav");
            let transport = Transport::new(channels, std::hint::spin_loop).unwrap();
            let tap = RecorderTap::new(Arc::clone(&transport));
            let status = Arc::new(Mutex::new(RecorderStatus::default()));
            let mut worker = Worker::new(transport, Arc::clone(&status), files.clone()).unwrap();
            tap.capture(&[], 48_000);
            worker
                .record(&RecorderRequest {
                    action: RecorderAction::Record,
                    path: Some(path.to_str().unwrap().into()),
                    sample_rate: None,
                    encoding: Some(encoding),
                })
                .unwrap();
            tap.capture(&vec![0.25; 3 * usize::from(channels)], 48_000);
            let (sender, receiver) = std::sync::mpsc::channel();
            drop(sender);
            crate::threaded_recording::run(worker, &receiver).unwrap();
            let stopped = status.lock().clone();
            assert_eq!((stopped.state, stopped.frames), (RecorderState::Stopped, 3));
            let mut reader = files.reader(&path);
            assert_eq!((reader.duration(), reader.spec().channels), (3, channels));
            if encoding == Encoding::Float32 {
                assert!(
                    reader
                        .samples::<f32>()
                        .all(|sample| sample.unwrap().to_bits() == 0.25_f32.to_bits())
                );
            } else {
                assert!(reader.samples::<i32>().all(|sample| sample.unwrap() > 0));
            }
            tap.capture(&vec![0.5; usize::from(channels)], 48_000);
            assert_eq!(
                mixrelay_test_support::rf64::inspect_bytes(&files.bytes(path))
                    .unwrap()
                    .frames,
                3
            );
        }
    }
}

#[test]
fn an_overlapping_fault_keeps_every_accepted_frame() {
    let directory = Path::new("/recorder-overlap-prefix");
    let files = MemoryFiles::default();
    let path = directory.join("take.wav");
    let transport = Transport::new(2, std::hint::spin_loop).unwrap();
    let tap = RecorderTap::new(Arc::clone(&transport));
    let status = Arc::new(Mutex::new(RecorderStatus::default()));
    let mut worker =
        Worker::new(Arc::clone(&transport), Arc::clone(&status), files.clone()).unwrap();
    tap.capture(&[], 48_000);
    worker
        .record(&RecorderRequest {
            action: RecorderAction::Record,
            path: Some(path.to_str().unwrap().into()),
            sample_rate: None,
            encoding: Some(Encoding::Float32),
        })
        .unwrap();
    for _ in 0..93 {
        tap.capture(&[0.5; 512 * 2], 48_000);
        worker.drain().unwrap();
    }
    // An overlapping producer faults before the last accepted packet is drained.
    tap.capture(&[0.5; 512 * 2], 48_000);
    transport.gate.fetch_or(
        (crate::transport::MAX_TOKEN + 1) * 3,
        std::sync::atomic::Ordering::SeqCst,
    );
    worker.service();
    assert_eq!(status.lock().state, RecorderState::Failed);
    assert!(
        status
            .lock()
            .error
            .as_ref()
            .unwrap()
            .contains("overlapping")
    );
    assert!(worker.take.is_none());
    assert_eq!(
        files.reader(&path).duration(),
        48_128,
        "every accepted frame is written before the fault is reported"
    );
}

mod failures;
