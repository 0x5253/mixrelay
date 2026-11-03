//! Graph worker diagnostics at the ownership boundary.
use super::*;
use std::sync::mpsc;

#[test]
fn shutdown_reports_the_final_partial_window() {
    let (mut runtime, received) = observed_runtime();
    let allocation = allocation_counter::measure(|| {
        runtime.record(Path::SharedSource);
        runtime.record(Path::SharedSource);
        runtime.record(Path::SharedSource);
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(allocation.bytes_total, 0);
    runtime.finish();
    assert_eq!(
        received
            .try_iter()
            .map(|counts| counts.shared_source)
            .sum::<u64>(),
        3
    );
}

#[test]
fn dropping_the_worker_drains_its_reporting_owner() {
    let (mut runtime, received) = observed_runtime();
    runtime.record(Path::SharedSource);
    drop(runtime);
    assert_eq!(
        received
            .try_iter()
            .map(|counts| counts.shared_source)
            .sum::<u64>(),
        1
    );
}

#[test]
fn time_read_misses_reach_the_reported_window_by_reason() {
    let (mut runtime, received) = observed_runtime();
    let allocation = allocation_counter::measure(|| {
        runtime.record_time_reads(TimeReadMisses {
            ahead: 1,
            reacquisitions: 1,
            ..TimeReadMisses::default()
        });
        runtime.record_time_reads(TimeReadMisses {
            not_ready: 3,
            behind: 2,
            mismatch: 1,
            ..TimeReadMisses::default()
        });
    });
    assert_eq!(allocation.count_total, 0);
    runtime.finish();
    let mut reported = TimeReadMisses::default();
    for counts in received.try_iter() {
        reported += counts.time_reads;
    }
    assert_eq!(
        reported,
        TimeReadMisses {
            not_ready: 3,
            ahead: 1,
            behind: 2,
            mismatch: 1,
            reacquisitions: 1,
            ..TimeReadMisses::default()
        }
    );
}

fn observed_runtime() -> (WorkerRuntime, mpsc::Receiver<Counts>) {
    let (sender, received) = mpsc::channel();
    let runtime = WorkerRuntime {
        realtime: None,
        attempted: false,
        period: Duration::ZERO,
        wake: None,
        timebase: Timebase::current(),
        latency: LatencyHistogram::default(),
        counts: Counts::default(),
        reported: Instant::now(),
        reporter: Some(
            Reporter::start(move |window| {
                sender.send(window.counts).unwrap();
            })
            .unwrap(),
        ),
    };
    (runtime, received)
}
