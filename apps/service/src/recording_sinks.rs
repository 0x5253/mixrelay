//! Connects the engine's recording ports to the file recorder sessions.
use mixrelay_audio_engine::{RecordingSink, RecordingSinkError, RecordingSinks};
use mixrelay_audio_recorder::{RecorderRegistry, RecorderTap};

/// The recorder registry as the engine's recording sinks.
pub(super) struct FileRecordingSinks<'a>(pub(super) &'a RecorderRegistry);

struct TapSink(RecorderTap);

impl RecordingSink for TapSink {
    fn capture(&self, samples: &[f32], sample_rate: u32) {
        self.0.capture(samples, sample_rate);
    }
}

impl RecordingSinks for FileRecordingSinks<'_> {
    fn prepare(
        &self,
        session: &str,
        channels: u16,
    ) -> Result<Box<dyn RecordingSink>, RecordingSinkError> {
        match self.0.prepare(session, channels) {
            Ok(tap) => Ok(Box::new(TapSink(tap))),
            Err(reason) => Err(RecordingSinkError(reason.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Take lifetimes across graph rebuilds: these depend on the recorder's tap
    //! accounting, so they run against real sessions and files.
    use std::sync::Arc;

    use mixrelay_audio_engine::{BlockProcessor, RenderPlan, recording_session_key};
    use mixrelay_audio_graph::{
        CompileContext, CompileLimits, GraphCompiler, GraphDocument, NodeConfig, NodeControl,
        NodeId, NodeSpec, RenderFormat,
    };
    use mixrelay_audio_io::recording::{RecorderAction, RecorderRequest, RecorderState};

    use super::*;

    fn recorder(registry: &RecorderRegistry, channels: u16) -> BlockProcessor {
        let document = GraphDocument::new(
            1,
            vec![NodeSpec {
                id: NodeId::from("record"),
                label: None,
                control: NodeControl::default(),
                config: NodeConfig::Recorder { channels },
            }],
            vec![],
        );
        let execution = GraphCompiler::default()
            .compile(
                &document,
                CompileContext {
                    format: RenderFormat {
                        sample_rate: 48_000,
                        block_frames: 4,
                    },
                    limits: CompileLimits::default(),
                },
            )
            .unwrap();
        let mut processor =
            BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap();
        processor
            .attach_recorders(&FileRecordingSinks(registry))
            .unwrap();
        processor
    }

    fn key(channels: u16) -> String {
        recording_session_key(&NodeId::from("record"), channels)
    }

    fn request(action: RecorderAction, path: Option<&std::path::Path>) -> RecorderRequest {
        RecorderRequest {
            action,
            path: path.map(|path| path.to_string_lossy().into_owned()),
            sample_rate: None,
            encoding: None,
        }
    }

    #[test]
    fn a_refused_session_is_a_sink_error() {
        let registry = RecorderRegistry::threaded_files();
        let error = FileRecordingSinks(&registry).prepare("", 2).err();
        assert!(error.is_some_and(|RecordingSinkError(reason)| !reason.is_empty()));
    }

    #[test]
    fn channel_change_can_be_cancelled_then_committed_without_losing_previous_transport() {
        let directory = mixrelay_test_support::FixtureDir::new("recording-channels").unwrap();
        let registry = RecorderRegistry::threaded_files();
        let mut previous = recorder(&registry, 2);
        drop(recorder(&registry, 8));
        registry.reap_unused().unwrap();
        previous.process_block(2, &[], &mut []).unwrap();
        let old_path = directory.join("previous.wav");
        registry
            .command(&key(2), request(RecorderAction::Record, Some(&old_path)))
            .unwrap();
        previous.process_block(2, &[], &mut []).unwrap();
        registry
            .command(&key(2), request(RecorderAction::Stop, None))
            .unwrap();
        let old_file = mixrelay_test_support::rf64::open(old_path).unwrap();
        assert_eq!(old_file.spec().channels, 2);
        assert_eq!(old_file.duration(), 2);
        assert!(
            registry
                .command(&key(8), request(RecorderAction::Stop, None))
                .is_err()
        );

        let mut accepted = recorder(&registry, 8);
        drop(previous);
        registry.reap_unused().unwrap();
        accepted.process_block(2, &[], &mut []).unwrap();
        let new_path = directory.join("eight.wav");
        registry
            .command(&key(8), request(RecorderAction::Record, Some(&new_path)))
            .unwrap();
        accepted.process_block(2, &[], &mut []).unwrap();
        registry
            .command(&key(8), request(RecorderAction::Stop, None))
            .unwrap();
        let new_file = mixrelay_test_support::rf64::open(new_path).unwrap();
        assert_eq!(new_file.spec().channels, 8);
        assert_eq!(new_file.duration(), 2);
    }

    #[test]
    fn rebuild_and_pause_preserve_take_and_final_tap_removal_finalizes_it() {
        let directory = mixrelay_test_support::FixtureDir::new("recording-rebuild").unwrap();
        let path = directory.join("take.wav");
        let registry = RecorderRegistry::threaded_files();
        let mut current = recorder(&registry, 2);
        current.process_block(2, &[], &mut []).unwrap();
        registry
            .command(&key(2), request(RecorderAction::Record, Some(&path)))
            .unwrap();
        // The adapter adds nothing to the audio thread: recording stays allocation-free.
        let measured = allocation_counter::measure(|| {
            current.process_block(2, &[], &mut []).unwrap();
        });
        assert_eq!(measured.count_total, 0);
        let mut replacement = recorder(&registry, 2);
        drop(current);
        registry.reap_unused().unwrap();
        assert_eq!(registry.status(&key(2)).state, RecorderState::Recording);
        registry
            .command(&key(2), request(RecorderAction::Pause, None))
            .unwrap();
        replacement.process_block(2, &[], &mut []).unwrap();
        registry
            .command(&key(2), request(RecorderAction::Record, None))
            .unwrap();
        replacement.process_block(2, &[], &mut []).unwrap();
        drop(replacement);
        registry.reap_unused().unwrap();
        let reader = mixrelay_test_support::rf64::open(path).unwrap();
        assert_eq!(reader.duration(), 4);
        assert_eq!(reader.spec().sample_rate, 48_000);
    }
}
