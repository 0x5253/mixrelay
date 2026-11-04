//! Exercise all product formats through persisted session settings and external
//! DAW adoption, without changing the user's device or preferences.
use super::*;
use mixrelay_audio_io::{
    formats::{BUFFER_SIZES, SAMPLE_RATES},
    session::AudioBackendKind,
};
fn runtime(endpoint: &Arc<AtomicU32>) -> AdapterRuntimeStatus {
    let mut profile = tests::profile(AudioBackendKind::CoreAudio);
    profile.initial_format.sample_rate = 48_000;
    profile.capabilities.supported_sample_rates = SAMPLE_RATES.to_vec();
    profile.capabilities.supported_block_frames = BUFFER_SIZES.to_vec();
    profile.capabilities.can_reconfigure_live = true;
    let endpoint = endpoint.clone();
    AdapterRuntimeStatus::new(profile, std::sync::Arc::new(mixrelay_time::StdClock::new()))
        .with_graph_format_control()
        .with_observed_rate_following(move || {
            let rate = endpoint.load(Ordering::Acquire);
            (rate != 0).then_some(rate)
        })
}

struct Store(mixrelay_test_support::FixtureDir);
impl Store {
    fn new() -> Self {
        Self(mixrelay_test_support::FixtureDir::new("session-catalog").unwrap())
    }
    fn preferences(&self) -> SessionPreferences {
        SessionPreferences::at(self.0.join("session.json"), AudioBackendKind::CoreAudio)
    }
}

#[test]
fn every_catalog_pair_persists_restores_and_follows_external_rate() {
    let endpoint = Arc::new(AtomicU32::new(48_000));
    let store = Store::new();
    let mut configured = runtime(&endpoint);
    configured.test_set_preferences(Ok(std::sync::Arc::new(store.preferences())));
    let configured = Arc::new(configured);
    let worker_status = configured.clone();
    let worker = std::thread::spawn(move || {
        while !worker_status.stop_requested() {
            let _ = worker_status.checkpoint();
            std::thread::park_timeout(Duration::from_millis(1));
        }
    });
    let _worker = AudioWorker::new(configured.clone(), worker);
    for &rate in SAMPLE_RATES {
        for &frames in BUFFER_SIZES {
            let requested = RenderFormat {
                sample_rate: rate,
                block_frames: frames,
            };
            configured.reconfigure(requested).unwrap();
            configured.activate_format(requested);
            configured.resume();
            let saved = SessionFormat {
                sample_rate: rate,
                block_frames: frames,
            };
            assert_eq!(store.preferences().load().unwrap().unwrap(), saved);
            // A restart restores the saved pair while the endpoint still runs at that rate.
            endpoint.store(rate, Ordering::Release);
            let fallback = RenderFormat {
                sample_rate: 48_000,
                block_frames: 128,
            };
            assert_eq!(
                runtime(&endpoint)
                    .restore_preferences(Ok(std::sync::Arc::new(store.preferences()))),
                requested
            );
            let external_rate = if rate == 192_000 { 44_100 } else { 192_000 };
            endpoint.store(external_rate, Ordering::Release);
            let external = RenderFormat {
                sample_rate: external_rate,
                block_frames: frames,
            };
            configured.adopt_external_format(external).unwrap();
            configured.activate_format(external);
            configured.resume();
            assert_eq!(endpoint.load(Ordering::Acquire), external_rate);
            assert_eq!(
                store.preferences().load().unwrap().unwrap(),
                SessionFormat {
                    sample_rate: external_rate,
                    block_frames: frames
                }
            );
            assert_ne!(fallback, external);
        }
    }
}
