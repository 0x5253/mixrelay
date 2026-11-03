//! Same-thread allocation accounting, including failure and deferred reclamation.
//! This measures owned Rust storage, not native plug-in or installed-app memory.
use super::*;
use crate::{ProcessorConfigurationError, ProcessorExchange, ProcessorSubmitError};
use mixrelay_audio_graph::{CompressorConfig, LimiterConfig};

struct Measurement;

impl Measurement {
    fn start() -> Self {
        HEAP_BALANCE.with(|count| {
            assert!(count.get().is_none(), "nested heap measurement");
            count.set(Some(HeapBalance::default()));
        });
        Self
    }

    fn finish(self) -> HeapBalance {
        let balance = HEAP_BALANCE.with(|count| count.replace(None).unwrap());
        drop(self);
        balance
    }
}

impl Drop for Measurement {
    fn drop(&mut self) {
        // Restore instrumentation after an assertion panic as well.
        HEAP_BALANCE.with(|count| count.set(None));
    }
}

fn assert_reclaimed(balance: HeapBalance) {
    assert!(balance.allocated_bytes > 0, "fixture did not allocate");
    assert_eq!(balance.allocated_bytes, balance.freed_bytes, "{balance:?}");
    assert_eq!(balance.allocations, balance.frees, "{balance:?}");
    eprintln!(
        "reclaimed {} bytes in {} allocations",
        balance.allocated_bytes, balance.allocations
    );
}

fn prepared(revision: u64, channels: u16, format: RenderFormat) -> Box<BlockProcessor> {
    let graph = GraphDocument::new(
        revision,
        vec![
            node(
                "source",
                NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::DefaultInput,
                    output_channels: channels,
                    gain_db: 0.0,
                }),
            ),
            node(
                "compressor",
                NodeConfig::Compressor(CompressorConfig {
                    version: 1,
                    bypass: false,
                    channels,
                    threshold_db: -18.0,
                    ratio: 4.0,
                    attack_ms: 10.0,
                    release_ms: 120.0,
                    makeup_gain_db: 0.0,
                }),
            ),
            node(
                "limiter",
                NodeConfig::Limiter(LimiterConfig {
                    version: 1,
                    bypass: false,
                    channels,
                    ceiling_db: -1.0,
                    release_ms: 80.0,
                }),
            ),
            node(
                "sink",
                NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::DefaultOutput,
                    input_channels: channels,
                }),
            ),
        ],
        vec![
            edge("a", "source", AUDIO_PORT_ID, "compressor", AUDIO_PORT_ID),
            edge("b", "compressor", AUDIO_PORT_ID, "limiter", AUDIO_PORT_ID),
            edge("c", "limiter", AUDIO_PORT_ID, "sink", AUDIO_PORT_ID),
        ],
    );
    // Exercise persistence storage as well as compilation and DSP buffers.
    let saved = serde_json::to_vec(&graph).unwrap();
    let restored = serde_json::from_slice(&saved).unwrap();
    let execution = GraphCompiler::default()
        .compile(
            &restored,
            CompileContext {
                format,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
}

#[test]
fn memory_preparation_and_restore_reclaim_every_owned_byte_across_formats() {
    // No background/native owners: every allocation and free occurs on this thread.
    let measurement = Measurement::start();
    for sample_rate in [44_100, 48_000, 96_000, 192_000] {
        for block_frames in [1, 128, 1_024] {
            for channels in [1, 2, 8, 64] {
                for _ in 0..8 {
                    let processor = prepared(
                        1,
                        channels,
                        RenderFormat {
                            sample_rate,
                            block_frames,
                        },
                    );
                    let plan_owner = Arc::downgrade(processor.plan());
                    let control_owner = Arc::downgrade(processor.plan().controls());
                    drop(processor);
                    assert!(plan_owner.upgrade().is_none());
                    assert!(control_owner.upgrade().is_none());
                }
            }
        }
    }
    assert_reclaimed(measurement.finish());
}

#[test]
fn memory_reconfiguration_retirement_and_shutdown_reclaim_every_owned_byte() {
    let measurement = Measurement::start();
    let exchange = ProcessorExchange::new(1);
    let mut current = prepared(1, 2, FORMAT);
    for revision in 2..=513 {
        let channels = [1, 2, 8, 64][usize::try_from(revision % 4).unwrap()];
        let next = prepared(
            revision,
            channels,
            RenderFormat {
                sample_rate: if revision % 2 == 0 { 48_000 } else { 96_000 },
                block_frames: if revision % 2 == 0 { 128 } else { 1_024 },
            },
        );
        let previous_owner = Arc::downgrade(current.plan());
        exchange.submit(next).unwrap();
        assert!(exchange.activate_pending(&mut current));
        // Audio retires storage; it is still alive until control explicitly reclaims it.
        assert!(previous_owner.upgrade().is_some());
        exchange.reclaim_retired();
        assert!(previous_owner.upgrade().is_none());
    }
    drop(current);
    drop(exchange);
    assert_reclaimed(measurement.finish());
}

#[test]
fn memory_failed_submission_activation_and_pending_shutdown_reclaim_storage() {
    let measurement = Measurement::start();
    for _ in 0..128 {
        let exchange = ProcessorExchange::new(1);
        let mut current = prepared(1, 64, FORMAT);
        let failed = prepared(2, 64, FORMAT);
        let failed_owner = Arc::downgrade(failed.plan());
        assert_eq!(
            exchange.submit_configured(failed, || Err("configuration failed")),
            Err(ProcessorConfigurationError::Configuration(
                "configuration failed"
            ))
        );
        assert!(failed_owner.upgrade().is_none());
        let stale = prepared(1, 64, FORMAT);
        let stale_owner = Arc::downgrade(stale.plan());
        assert_eq!(
            exchange.submit_configured(stale, || -> Result<(), &str> {
                panic!("stale submission must not run configuration")
            }),
            Err(ProcessorConfigurationError::StaleRevision {
                current: 1,
                proposed: 1,
            })
        );
        assert!(stale_owner.upgrade().is_none());
        let pending = prepared(2, 64, FORMAT);
        let pending_owner = Arc::downgrade(pending.plan());
        exchange.submit(pending).unwrap();
        let busy = prepared(3, 64, FORMAT);
        let busy_owner = Arc::downgrade(busy.plan());
        assert_eq!(
            exchange.submit(busy),
            Err(ProcessorSubmitError::Busy {
                proposed_revision: 3
            })
        );
        assert!(busy_owner.upgrade().is_none());
        assert_eq!(
            exchange.activate_pending_configured(&mut current, |_| Err("activation failed")),
            Err("activation failed")
        );
        assert_eq!(current.plan().revision(), 1);
        assert!(pending_owner.upgrade().is_some());
        drop(exchange);
        assert!(pending_owner.upgrade().is_none());
        drop(current);
    }
    assert_reclaimed(measurement.finish());
}

#[test]
fn memory_dynamics_render_and_activation_do_not_allocate_or_free() {
    let exchange = ProcessorExchange::new(1);
    let mut current = prepared(1, 64, FORMAT);
    let next = prepared(2, 64, FORMAT);
    exchange.submit(next).unwrap();
    let input: [f32; 256] = std::array::from_fn(|index| if index % 2 == 0 { 2.0 } else { -2.0 });
    let mut output = [0.0; 256];
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    assert!(exchange.activate_pending(&mut current));
    for index in 0..10_000 {
        let frames = [1, 3, 2, 4][index % 4];
        let samples = usize::try_from(frames).unwrap() * 64;
        current
            .process_block(frames, &[&input[..samples]], &mut [&mut output[..samples]])
            .unwrap();
        assert!(output[..samples].iter().all(|sample| sample.is_finite()));
        assert!(
            output[..samples]
                .iter()
                .all(|sample| sample.abs() <= 0.891_251)
        );
    }
    let operations = HEAP_OPERATIONS.with(|count| count.replace(None));
    assert_eq!(operations, Some(0));
    exchange.reclaim_retired();
}

#[test]
fn memory_every_builtin_reconfiguration_and_failed_submission_reclaims_storage() {
    let measurement = Measurement::start();
    for config in super::builtin_effects::configs() {
        let exchange = ProcessorExchange::new(1);
        let mut current = super::builtin_effects::processor(config.clone(), 1, FORMAT);
        for revision in 2..34 {
            exchange
                .submit(super::builtin_effects::processor(
                    config.clone(),
                    revision,
                    FORMAT,
                ))
                .unwrap();
            assert!(exchange.activate_pending(&mut current));
            exchange.reclaim_retired();
            assert!(
                exchange
                    .submit_configured(
                        super::builtin_effects::processor(config.clone(), revision + 1, FORMAT),
                        || Err("failed")
                    )
                    .is_err()
            );
        }
        drop(current);
        drop(exchange);
    }
    assert_reclaimed(measurement.finish());
}
