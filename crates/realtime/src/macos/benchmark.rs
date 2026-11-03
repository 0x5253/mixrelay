//! Run alone in release mode: `cargo test --release -p mixrelay-realtime
//! macos::benchmark -- --nocapture --test-threads=1`. A realtime producer releases a
//! numbered publication every 128 frames at 48 kHz, like a device IO cycle, recording
//! its time in a preallocated ring. The consumer measures every publication's delay,
//! including ones it slept through, comparing today's normal-priority 250 µs polling
//! with a realtime consumer woken by a [`Notifier`] (combined treatment).
use super::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

const PERIOD: Duration = Duration::from_micros(2_667);
const CYCLES: usize = 3_000;

struct Shared {
    published_at: Vec<AtomicU64>,
    sequence: AtomicU64,
    ready: AtomicBool,
    notifier: Notifier,
}

fn run(realtime: bool) -> Vec<u64> {
    let shared = Arc::new(Shared {
        published_at: (0..CYCLES).map(|_| AtomicU64::new(0)).collect(),
        sequence: AtomicU64::new(0),
        ready: AtomicBool::new(false),
        notifier: Notifier::new().unwrap(),
    });
    let timebase = Timebase::current();
    let consumer = {
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            if realtime {
                promote_current_thread(PERIOD, PERIOD * 2 / 5, PERIOD * 4 / 5).unwrap();
            }
            let mut latencies = Vec::with_capacity(CYCLES);
            let mut seen = 0;
            shared.ready.store(true, Ordering::Release);
            // Drain to the final publication: shutdown never counts as a miss.
            while seen < CYCLES {
                let available = usize::try_from(shared.sequence.load(Ordering::Acquire)).unwrap();
                if available > seen {
                    let now = host_time();
                    for published in &shared.published_at[seen..available] {
                        let at = published.load(Ordering::Relaxed);
                        latencies.push(timebase.nanoseconds(now.saturating_sub(at)));
                    }
                    seen = available;
                } else if realtime {
                    let _ = shared.notifier.wait(PERIOD * 2);
                } else {
                    thread::sleep(Duration::from_micros(250));
                }
            }
            latencies
        })
    };
    while !shared.ready.load(Ordering::Acquire) {
        // A consumer that failed to start must fail the run, not hang it.
        assert!(!consumer.is_finished(), "consumer failed to start");
        thread::yield_now();
    }
    let producer = Arc::clone(&shared);
    thread::spawn(move || {
        promote_current_thread(PERIOD, PERIOD / 5, PERIOD / 2).unwrap();
        let period = timebase.ticks(PERIOD);
        let mut release = host_time() + period;
        for index in 0..CYCLES {
            wait_until(release);
            producer.published_at[index].store(host_time(), Ordering::Relaxed);
            producer
                .sequence
                .store(u64::try_from(index + 1).unwrap(), Ordering::Release);
            producer.notifier.notify();
            release += period;
        }
    })
    .join()
    .unwrap();
    consumer.join().unwrap()
}

fn report(name: &str, mut latencies: Vec<u64>) {
    let period = u64::try_from(PERIOD.as_nanos()).unwrap();
    let late = latencies
        .iter()
        .filter(|&&latency| latency > period)
        .count();
    latencies.sort_unstable();
    // Nearest-rank percentile in tenths of a percent, shown in µs with one decimal.
    let at = |permille: usize| {
        let nanoseconds = latencies[(latencies.len() - 1) * permille / 1_000];
        format!("{}.{}", nanoseconds / 1_000, nanoseconds % 1_000 / 100)
    };
    eprintln!(
        "{name}: n={} delay µs p50={} p99={} p99.9={} max={} later-than-one-period={late}",
        latencies.len(),
        at(500),
        at(990),
        at(999),
        at(1_000),
    );
}

#[test]
fn wake_latency() {
    report("sleep 250µs polling", run(false));
    report("realtime + notifier", run(true));
}
