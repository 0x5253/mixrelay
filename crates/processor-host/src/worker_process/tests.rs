//! Process-fault reporting retains the worker boundary and reports only new events.

use std::io::Write;

#[cfg(unix)]
mod isolation;

use super::*;
use crate::{
    AudioLayout,
    process_fault::{FaultKind, FaultObserver},
    shared_transport,
};

#[test]
fn supervision_observes_editor_and_blob_requests_until_the_worker_acknowledges_them() {
    let (_, mailbox, _, worker) = shared_transport::memory::endpoints(
        AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    );
    let control =
        HostedProcessorControl::native(mailbox, Arc::new(mixrelay_time::ManualClock::default()));
    assert!(!native_activity(&control).parent_request_pending);
    control.set_editor_open(true);
    assert!(native_activity(&control).parent_request_pending);
    let header = worker.header();
    header.editor_consumed.store(
        header.editor_sequence.load(Ordering::Acquire),
        Ordering::Release,
    );
    assert!(!native_activity(&control).parent_request_pending);
    header.worker_status.store(WORKER_READY, Ordering::Release);
    assert_eq!(
        control.request_blob(crate::editor_surface::EDITOR_FRAME, Duration::ZERO),
        Err(crate::ProcessorStateError::Timeout),
    );
    assert!(native_activity(&control).parent_request_pending);
    let (request, _) = worker.pending_request().unwrap();
    worker.publish(request, Err(crate::ProcessorStateError::Timeout));
    assert!(!native_activity(&control).parent_request_pending);
}

#[derive(Clone)]
struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn repeated_monitor_windows_report_only_new_faults_with_their_owner() {
    let (_, mailbox, _, _) = shared_transport::memory::endpoints(
        AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    );
    let control = HostedProcessorControl::native(
        mailbox,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    );
    let instance = control.shared.header().instance_id;
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = LogWriter(Arc::clone(&bytes));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let mut faults = FaultObserver::default();
        control
            .shared
            .header()
            .process_faults
            .record(FaultKind::NonFinitePcm);
        report_process_faults(&control, &mut faults);
        for _ in 0..32 {
            report_process_faults(&control, &mut faults);
        }
        control
            .shared
            .header()
            .process_faults
            .record(FaultKind::NonFinitePcm);
        report_process_faults(&control, &mut faults);
    });
    let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    for line in &lines {
        for field in [
            format!("os=\"{}\"", std::env::consts::OS),
            "boundary=\"native_processor\"".to_owned(),
            "native_format=\"unknown\"".to_owned(),
            "phase=\"process\"".to_owned(),
            "cause=\"nonfinite_pcm\"".to_owned(),
            format!("instance_id={instance}"),
            "new_failures=1".to_owned(),
        ] {
            assert!(line.contains(&field), "missing {field}: {line}");
        }
    }
    assert!(lines[0].contains("failures=1"));
    assert!(lines[1].contains("failures=2"));
}

#[test]
fn shared_native_details_reach_the_parent_once_with_format_and_raw_status() {
    use truce_rack_core::{Error, ProcessFault};
    for (format, fault) in [
        (crate::RackFormat::Au, ProcessFault::NativeStatus(-10_867)),
        (crate::RackFormat::Vst3, ProcessFault::InvalidAudioMapping),
        (crate::RackFormat::Clap, ProcessFault::MissingCallback),
        (crate::RackFormat::Lv2, ProcessFault::OutputEventCapacity),
    ] {
        let (_, mailbox, _, _) = shared_transport::memory::endpoints(
            AudioLayout {
                inputs: 2,
                outputs: 2,
                block_frames: 4,
            },
            crate::instance_id::InstanceId::new(1).unwrap(),
        );
        let control = HostedProcessorControl::native(
            mailbox,
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        )
        .with_format(format);
        let faults = &control.shared.header().process_faults;
        let measured = allocation_counter::measure(|| {
            faults.record_native(&Error::Process {
                format: format.argument(),
                fault,
            });
            faults.record(FaultKind::NativeCall);
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = LogWriter(Arc::clone(&bytes));
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let mut observer = FaultObserver::default();
            for _ in 0..32 {
                report_process_faults(&control, &mut observer);
            }
            faults.record_native(&Error::Process {
                format: format.argument(),
                fault: ProcessFault::InvalidBlockSize,
            });
            faults.record(FaultKind::NativeCall);
            report_process_faults(&control, &mut observer);
        });
        let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        let details = text
            .lines()
            .filter(|line| line.contains("first_native_process_failure"))
            .collect::<Vec<_>>();
        assert_eq!(details.len(), 1, "{text}");
        let line = details[0];
        assert!(
            line.contains(&format!("native_format=\"{}\"", format.argument())),
            "{line}"
        );
        assert!(line.contains(&format!("fault={fault:?}")), "{line}");
        if let ProcessFault::NativeStatus(code) = fault {
            assert!(line.contains(&format!("native_code={code}")), "{line}");
        }
        assert!(
            line.contains(&format!(
                "instance_id={}",
                control.shared.header().instance_id
            )),
            "{line}"
        );
        assert_eq!(text.lines().count(), 3, "{text}");
    }
}
