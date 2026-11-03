use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;

use super::*;

/// What one fake sink received. Capacity is reserved up front so capture stays
/// allocation-free inside measured callbacks; locking is fine in this single-threaded fake.
#[derive(Debug)]
struct SinkLog {
    samples: Mutex<Vec<f32>>,
    sample_rate: AtomicU32,
}

#[derive(Debug)]
struct LoggedSink(Arc<SinkLog>);

impl RecordingSink for LoggedSink {
    fn capture(&self, samples: &[f32], sample_rate: u32) {
        self.0.samples.lock().extend_from_slice(samples);
        self.0.sample_rate.store(sample_rate, Ordering::Relaxed);
    }
}

struct PreparedSink {
    session: String,
    channels: u16,
    log: Arc<SinkLog>,
}

#[derive(Default)]
struct FakeSinks {
    prepared: Mutex<Vec<PreparedSink>>,
    refusal: Option<RecordingSinkError>,
}

impl FakeSinks {
    fn only_log(&self) -> Arc<SinkLog> {
        let prepared = self.prepared.lock();
        assert_eq!(prepared.len(), 1);
        Arc::clone(&prepared[0].log)
    }
}

impl RecordingSinks for FakeSinks {
    fn prepare(
        &self,
        session: &str,
        channels: u16,
    ) -> Result<Box<dyn RecordingSink>, RecordingSinkError> {
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        let log = Arc::new(SinkLog {
            samples: Mutex::new(Vec::with_capacity(4096)),
            sample_rate: AtomicU32::new(0),
        });
        self.prepared.lock().push(PreparedSink {
            session: session.to_owned(),
            channels,
            log: Arc::clone(&log),
        });
        Ok(Box::new(LoggedSink(log)))
    }
}

fn captured(log: &SinkLog) -> Vec<f32> {
    log.samples.lock().clone()
}

fn recorder_plan() -> Arc<RenderPlan> {
    let execution = compile(
        vec![
            device_input("mic"),
            device_output("monitor"),
            node("record", NodeConfig::Recorder { channels: 2 }),
        ],
        vec![
            edge("listen", "mic", AUDIO_PORT_ID, "monitor", AUDIO_PORT_ID),
            edge("save", "mic", AUDIO_PORT_ID, "record", AUDIO_PORT_ID),
        ],
    );
    Arc::new(RenderPlan::prepare(&execution).unwrap())
}

#[test]
fn each_recorder_node_gets_a_sink_keyed_by_node_and_channel_count() {
    let execution = compile(
        vec![
            node("stereo", NodeConfig::Recorder { channels: 2 }),
            node("wide", NodeConfig::Recorder { channels: 8 }),
        ],
        vec![],
    );
    let sinks = FakeSinks::default();
    let mut processor =
        BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap();
    processor.attach_recorders(&sinks).unwrap();
    let mut prepared: Vec<_> = sinks
        .prepared
        .lock()
        .iter()
        .map(|sink| (sink.session.clone(), sink.channels))
        .collect();
    prepared.sort();
    assert_eq!(
        prepared,
        [
            (recording_session_key(&NodeId::from("stereo"), 2), 2),
            (recording_session_key(&NodeId::from("wide"), 8), 8),
        ]
    );
}

#[test]
fn a_refused_sink_fails_preparation_with_its_diagnostic() {
    let sinks = FakeSinks {
        refusal: Some(RecordingSinkError("session limit reached".into())),
        ..FakeSinks::default()
    };
    let mut processor = BlockProcessor::new(recorder_plan()).unwrap();
    assert_eq!(
        processor.attach_recorders(&sinks).unwrap_err(),
        RecordingSinkError("session limit reached".into())
    );
}

#[test]
fn recorder_captures_final_routing_without_native_sink_or_callback_allocation() {
    let sinks = FakeSinks::default();
    let plan = recorder_plan();
    // Only the real monitor is a native output. A recording has no hardware clock.
    assert_eq!(plan.sink_bindings().len(), 1);
    let mut processor = BlockProcessor::new(plan).unwrap();
    processor.attach_recorders(&sinks).unwrap();
    let log = sinks.only_log();
    let input = [0.25, -0.25, 0.5, -0.5];
    let mut output = [0.0; 4];
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor
        .process_block(2, &[&input], &mut [&mut output])
        .unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(input.map(f32::to_bits), output.map(f32::to_bits));
    assert_eq!(captured(&log), input);
    assert_eq!(log.sample_rate.load(Ordering::Relaxed), 48_000);
}

#[test]
fn mixed_sources_record_without_any_hardware_output() {
    let execution = compile(
        vec![
            device_input("a"),
            device_input("b"),
            node("record", NodeConfig::Recorder { channels: 2 }),
        ],
        vec![
            edge("a-record", "a", AUDIO_PORT_ID, "record", AUDIO_PORT_ID),
            edge("b-record", "b", AUDIO_PORT_ID, "record", AUDIO_PORT_ID),
        ],
    );
    let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
    assert!(plan.sink_bindings().is_empty());
    let sinks = FakeSinks::default();
    let mut processor = BlockProcessor::new(plan).unwrap();
    processor.attach_recorders(&sinks).unwrap();
    let left = [0.125, -0.25, 0.25, 0.5];
    let right = [0.125, 0.5, -0.5, 0.25];
    processor
        .process_block(2, &[&left, &right], &mut [])
        .unwrap();
    let expected: Vec<f32> = left.iter().zip(right).map(|(a, b)| a + b).collect();
    assert_eq!(captured(&sinks.only_log()), expected);
}

#[test]
fn muted_recorder_captures_silence_instead_of_skipping_blocks() {
    let mut recorder = node("record", NodeConfig::Recorder { channels: 2 });
    recorder.control.muted = true;
    let execution = compile(
        vec![device_input("in"), recorder],
        vec![edge("record", "in", AUDIO_PORT_ID, "record", AUDIO_PORT_ID)],
    );
    let sinks = FakeSinks::default();
    let mut processor =
        BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap();
    processor.attach_recorders(&sinks).unwrap();
    processor.process_block(2, &[&[1.0; 4]], &mut []).unwrap();
    let samples = captured(&sinks.only_log());
    assert_eq!(samples.len(), 4);
    assert!(
        samples
            .iter()
            .all(|sample| sample.to_bits() == 0.0_f32.to_bits())
    );
}

#[test]
fn control_publications_keep_capturing_and_off_skips_capture_instead_of_writing_silence() {
    let sinks = FakeSinks::default();
    let plan = recorder_plan();
    let mut processor = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    processor.attach_recorders(&sinks).unwrap();
    let log = sinks.only_log();
    let input = [0.25; 4];
    let mut output = [0.0; 4];
    let exchange = crate::ProcessorExchange::new(1);
    for revision in 2..102 {
        exchange
            .publish_controls(
                plan.controls(),
                revision,
                &[crate::RuntimeControlUpdate {
                    node_id: NodeId::from("monitor"),
                    control: mixrelay_audio_graph::NodeControl {
                        enabled: revision % 2 == 0,
                        muted: false,
                    },
                    gain_db: None,
                }],
            )
            .unwrap();
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        processor
            .process_block(2, &[&input], &mut [&mut output])
            .unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    }
    for (revision, enabled, muted) in [(102, false, false), (103, true, true)] {
        exchange
            .publish_controls(
                plan.controls(),
                revision,
                &[crate::RuntimeControlUpdate {
                    node_id: NodeId::from("record"),
                    control: mixrelay_audio_graph::NodeControl { enabled, muted },
                    gain_db: None,
                }],
            )
            .unwrap();
        processor
            .process_block(2, &[&input], &mut [&mut output])
            .unwrap();
    }
    assert!(Arc::ptr_eq(processor.plan(), &plan));
    // 100 monitored blocks, nothing while OFF, then one muted block.
    let samples = captured(&log);
    assert_eq!(samples.len(), 404);
    assert!(
        samples[..400]
            .iter()
            .all(|sample| (*sample - 0.25).abs() < f32::EPSILON)
    );
    assert!(
        samples[400..]
            .iter()
            .all(|sample| sample.abs() < f32::EPSILON)
    );
}
