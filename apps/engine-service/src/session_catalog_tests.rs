//! Exercise all product formats through persistent native-rate settings and
//! external DAW adoption, without changing the user's device or preferences.
use super::*;
use mixrelay_audio_io::{
    formats::{BUFFER_SIZES, SAMPLE_RATES},
    session::AudioBackendKind,
};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());
static RATE: AtomicU32 = AtomicU32::new(48_000);
static WRITES: AtomicU32 = AtomicU32::new(0);
fn set_rate(rate: u32) -> Result<(), String> {
    if !SAMPLE_RATES.contains(&rate) {
        return Err("outside product catalog".into());
    }
    WRITES.fetch_add(1, Ordering::Relaxed);
    RATE.store(rate, Ordering::Release);
    Ok(())
}
fn read_rate() -> Option<u32> {
    let rate = RATE.load(Ordering::Acquire);
    (rate != 0).then_some(rate)
}
fn runtime() -> AdapterRuntimeStatus {
    let mut profile = tests::profile(AudioBackendKind::CoreAudio);
    profile.initial_format.sample_rate = 48_000;
    profile.capabilities.supported_sample_rates = SAMPLE_RATES.to_vec();
    profile.capabilities.supported_block_frames = BUFFER_SIZES.to_vec();
    profile.capabilities.can_reconfigure_live = true;
    AdapterRuntimeStatus::new(profile, std::sync::Arc::new(mixrelay_time::StdClock::new()))
        .with_format_control(set_rate)
        .with_observed_rate_following(read_rate)
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
    let _guard = LOCK.lock().unwrap();
    let store = Store::new();
    let mut configured = runtime();
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
    let mut current = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    for &rate in SAMPLE_RATES {
        for &frames in BUFFER_SIZES {
            let requested = RenderFormat {
                sample_rate: rate,
                block_frames: frames,
            };
            configured.reconfigure(current, requested).unwrap();
            configured.activate_format(requested);
            configured.resume();
            let saved = store.preferences().load().unwrap().unwrap();
            assert_eq!(
                saved,
                SessionFormat {
                    sample_rate: rate,
                    block_frames: frames
                }
            );
            let restored = runtime();
            restored
                .test_restore_native_format(requested, current)
                .unwrap();
            assert_eq!(read_rate(), Some(rate));
            let external_rate = if rate == 192_000 { 44_100 } else { 192_000 };
            RATE.store(external_rate, Ordering::Release);
            let writes = WRITES.load(Ordering::Acquire);
            let external = RenderFormat {
                sample_rate: external_rate,
                block_frames: frames,
            };
            configured.adopt_external_format(external).unwrap();
            configured.activate_format(external);
            configured.resume();
            assert_eq!(
                WRITES.load(Ordering::Acquire),
                writes,
                "DAW adoption wrote back to native device"
            );
            assert_eq!(
                store.preferences().load().unwrap().unwrap(),
                SessionFormat {
                    sample_rate: external_rate,
                    block_frames: frames
                }
            );
            current = external;
        }
    }
}
