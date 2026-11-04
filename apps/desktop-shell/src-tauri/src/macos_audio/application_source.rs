//! One graph node's application source: its shared tap, the pump feeding the engine, and
//! an optional scoped playback mute.
use super::capture_backend::{
    FORWARDING_IDENTITY_REQUIRED, MacApplicationCaptureBackend, forwarding_device_uid,
};
use super::clock_pump::{self, PumpMailbox, ports::PumpPorts};
use super::process_tap::{ProcessTap, TapReader};
use super::scoped_playback::ScopedPlayback;
use super::source_transport::ApplicationSourceTransport;
use mixrelay_audio_graph::{
    ApplicationCaptureRequest, ApplicationSelector, NodeId, SourcePlaybackPolicy,
};
use mixrelay_audio_io::ApplicationCaptureBackend;
use mixrelay_time::StdClock;
use std::{
    sync::{Arc, Mutex, PoisonError, atomic::Ordering},
    thread::JoinHandle,
};

/// The production implementations a source pump runs with.
pub(super) enum NativePump {}

impl PumpPorts for NativePump {
    type Tap = TapReader;
    type Sink = ApplicationSourceTransport;
    type Lease = ScopedPlayback;
    type Clock = StdClock;
}

pub(super) struct ApplicationSource {
    capture: Arc<ProcessTap>,
    pump: Arc<PumpWorker>,
    /// A replacement tap's reader, published to the pump only once the engine accepts it.
    prepared_reader: Mutex<Option<TapReader>>,
    playback: Option<Arc<ScopedPlayback>>,
}

impl ApplicationSource {
    /// Prepares a node's source, reusing `previous` where the capture is unchanged. Nothing
    /// reaches the running pump until [`Self::activate`].
    pub(super) fn prepare(
        backend: &MacApplicationCaptureBackend,
        node_id: &NodeId,
        request: &ApplicationCaptureRequest,
        previous: Option<&Arc<Self>>,
    ) -> Result<Arc<Self>, String> {
        if previous.is_some_and(|source| {
            source.capture.resolved.stream.channels != request.stream.channels
        }) {
            return Err("live application transport channel replacement is unsupported".to_owned());
        }
        let playback = prepare_scoped_playback(request)?;
        if let Some(previous) = previous.filter(|previous| previous.captures(request)) {
            return Ok(Arc::new(Self {
                capture: Arc::clone(&previous.capture),
                pump: Arc::clone(&previous.pump),
                prepared_reader: Mutex::new(None),
                playback,
            }));
        }
        let capture = Arc::new(open_shared_capture(backend, request)?);
        // A replacement must reserve its sole reader before remote acceptance;
        // committing only moves an existing token and cannot fail acquisition.
        let reader = capture.acquire_reader()?;
        // A second connection to the same source would steal its producer lease
        // before HTTP acceptance. Keep the transport and its single reader alive;
        // only commit publishes the prepared tap to that pump's control mailbox.
        let (pump, prepared_reader) = match previous {
            Some(previous) => (Arc::clone(&previous.pump), Some(reader)),
            None => (
                Arc::new(PumpWorker::start(
                    node_id.as_str(),
                    reader,
                    request.stream.channels,
                )?),
                None,
            ),
        };
        Ok(Arc::new(Self {
            capture,
            pump,
            prepared_reader: Mutex::new(prepared_reader),
            playback,
        }))
    }

    /// Whether this source already captures what `request` selects.
    fn captures(&self, request: &ApplicationCaptureRequest) -> bool {
        mixrelay_macos_transport::capture_selection::source_id(&request.selector)
            .is_some_and(|id| id == self.capture.resolved.source_id)
    }

    /// Starts the scoped mute once the engine has started this source. Returns the lease
    /// to roll back if a later source fails; `None` when there is nothing to undo.
    pub(super) fn start_playback(&self) -> Result<Option<Arc<ScopedPlayback>>, String> {
        let Some(playback) = &self.playback else {
            return Ok(None);
        };
        playback
            .start()
            .map(|started| started.then(|| Arc::clone(playback)))
    }

    /// Hands a prepared replacement tap and the playback lease to the running pump.
    pub(super) fn activate(&self) {
        let reader = self
            .prepared_reader
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(reader) = reader {
            self.pump.mailbox.capture.publish(reader);
        }
        self.pump.mailbox.playback.publish(self.playback.clone());
    }
}

/// The exact app and `MixRelay` device whose direct playback a capture mutes, if any.
fn scoped_playback_target(
    request: &ApplicationCaptureRequest,
) -> Result<Option<(&str, String)>, String> {
    if request.source_playback != SourcePlaybackPolicy::MuteWhileCaptured {
        return Ok(None);
    }
    let ApplicationSelector::BundleId {
        bundle_id,
        include_descendants: false,
    } = &request.selector
    else {
        return Err("scoped playback requires an exact bundle selector".into());
    };
    let device_uid = forwarding_device_uid(request).ok_or(FORWARDING_IDENTITY_REQUIRED)?;
    Ok(Some((bundle_id, device_uid)))
}

fn prepare_scoped_playback(
    request: &ApplicationCaptureRequest,
) -> Result<Option<Arc<ScopedPlayback>>, String> {
    let Some((bundle_id, device_uid)) = scoped_playback_target(request)? else {
        return Ok(None);
    };
    let tap =
        ProcessTap::prepare(bundle_id, Some(&device_uid)).map_err(|error| error.to_string())?;
    Ok(Some(Arc::new(ScopedPlayback::new(tap))))
}

/// Opens the started, unmuted tap the pump reads; muting belongs to the scoped tap.
fn open_shared_capture(
    backend: &MacApplicationCaptureBackend,
    request: &ApplicationCaptureRequest,
) -> Result<ProcessTap, String> {
    let mut preserved = request.clone();
    preserved.source_playback = SourcePlaybackPolicy::Preserve;
    preserved.forwarding_endpoint = None;
    let tap = backend
        .open(&preserved)
        .map_err(|error| error.to_string())?;
    let stats = tap.stats();
    if stats.channels != u32::from(request.stream.channels) {
        return Err("application capture channel count does not match the graph".to_owned());
    }
    clock_pump::native_rate(stats.sample_rate)?;
    Ok(tap)
}

/// The thread running one source's pump; dropping it stops and joins the pump.
struct PumpWorker {
    mailbox: Arc<PumpMailbox<NativePump>>,
    worker: Option<JoinHandle<()>>,
}

impl PumpWorker {
    fn start(source_id: &str, capture: TapReader, channels: u16) -> Result<Self, String> {
        let transport = ApplicationSourceTransport::connect(source_id, channels)?;
        let mailbox = Arc::new(PumpMailbox::default());
        let worker_mailbox = Arc::clone(&mailbox);
        let worker = std::thread::Builder::new()
            .name(format!("mixrelay-source-{source_id}"))
            .spawn(move || {
                let result = clock_pump::run::<NativePump>(
                    capture,
                    transport,
                    usize::from(channels),
                    &worker_mailbox,
                    StdClock::new(),
                );
                if let Err(error) = result {
                    tracing::error!(%error, "application source clock bridge stopped");
                }
            })
            .map_err(|error| format!("could not start application source pump: {error}"))?;
        Ok(Self {
            mailbox,
            worker: Some(worker),
        })
    }
}

impl Drop for PumpWorker {
    fn drop(&mut self) {
        self.mailbox.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(selector: &serde_json::Value, endpoint: Option<&str>) -> ApplicationCaptureRequest {
        serde_json::from_value(serde_json::json!({
            "selector": selector,
            "stream": {"channels": 2, "channelLayout": "stereo"},
            "sourcePlayback": "mute_while_captured",
            "forwardingEndpoint": endpoint,
        }))
        .unwrap()
    }

    #[test]
    fn scoped_playback_needs_an_exact_app_and_an_owned_forwarding_endpoint() {
        let exact = serde_json::json!({
            "match": "bundle_id", "bundle_id": "org.example.Player", "include_descendants": false
        });
        let endpoint = "17e59d73-42c1-4f11-a8b4-6c2d913357e0";
        let mut muted = request(&exact, Some(endpoint));
        assert_eq!(
            scoped_playback_target(&muted),
            Ok(Some((
                "org.example.Player",
                "io.mixrelay.audio.endpoint.17e59d7342c14f11a8b46c2d913357e0".to_owned()
            )))
        );

        let preserved = &mut muted;
        preserved.source_playback = SourcePlaybackPolicy::Preserve;
        assert_eq!(scoped_playback_target(preserved), Ok(None));

        assert_eq!(
            scoped_playback_target(&request(&exact, None)),
            Err(FORWARDING_IDENTITY_REQUIRED.to_owned())
        );
        assert_eq!(
            scoped_playback_target(&request(&exact, Some("not-an-endpoint"))),
            Err(FORWARDING_IDENTITY_REQUIRED.to_owned())
        );
        let system = serde_json::json!({"match": "all_except", "bundle_ids": []});
        assert!(scoped_playback_target(&request(&system, Some(endpoint))).is_err());
    }
}
