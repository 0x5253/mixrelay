use super::*;
use std::sync::{Arc, Mutex};
use tracing::{Event, Metadata, Subscriber, field::Visit, span};

#[derive(Debug, Default, PartialEq, Eq)]
struct Receipt {
    procedure: String,
    event: String,
    code: u64,
    status: i64,
    call_id: u64,
}

impl Visit for Receipt {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "procedure" => value.clone_into(&mut self.procedure),
            "event" => value.clone_into(&mut self.event),
            _ => {}
        }
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        match field.name() {
            "code" => self.code = value,
            "call_id" => self.call_id = value,
            _ => {}
        }
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        if field.name() == "os_status" {
            self.status = value;
        }
    }
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
}

#[derive(Clone)]
struct Recorder(Arc<Mutex<Vec<Receipt>>>);

impl Subscriber for Recorder {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }
    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }
    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut receipt = Receipt::default();
        event.record(&mut receipt);
        self.0.lock().unwrap().push(receipt);
    }
    fn enter(&self, _: &span::Id) {}
    fn exit(&self, _: &span::Id) {}
}

#[test]
fn entered_receipt_precedes_native_action_and_return_preserves_its_error() {
    let records = Recorder(Arc::default());
    let error = TapError {
        code: 4,
        os_status: -10863,
    };
    tracing::subscriber::with_default(records.clone(), || {
        let result = run(Procedure::RegisterIo, || {
            assert_eq!(
                records
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|receipt| receipt.event.as_str())
                    .collect::<Vec<_>>(),
                ["entered"],
                "native action began before its entry receipt"
            );
            records.0.lock().unwrap().push(Receipt {
                event: "action".into(),
                ..Receipt::default()
            });
            Err::<(), _>(error)
        });
        assert_eq!(result, Err(error));
    });
    let receipts = records.0.lock().unwrap();
    assert_eq!(
        receipts
            .iter()
            .map(|receipt| receipt.event.as_str())
            .collect::<Vec<_>>(),
        ["entered", "action", "returned"]
    );
    assert_eq!(receipts[0].procedure, "AudioDeviceCreateIOProcID");
    assert_eq!(receipts[2].procedure, "AudioDeviceCreateIOProcID");
    assert_eq!(receipts[2].code, 4);
    assert_eq!(receipts[2].status, i64::from(error.os_status));
    assert_ne!(receipts[0].call_id, 0);
    assert_eq!(receipts[0].call_id, receipts[2].call_id);
}

#[test]
fn successful_control_return_reports_the_same_procedure_without_error() {
    let records = Recorder(Arc::default());
    let result = tracing::subscriber::with_default(records.clone(), || {
        run(Procedure::StartIo, || Ok(17_u32))
    });
    assert_eq!(result, Ok(17));
    let receipts = records.0.lock().unwrap();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].event, "entered");
    assert_eq!(receipts[1].event, "returned");
    assert_eq!(receipts[1].procedure, "AudioDeviceStart");
    assert_eq!((receipts[1].code, receipts[1].status), (0, 0));
}
