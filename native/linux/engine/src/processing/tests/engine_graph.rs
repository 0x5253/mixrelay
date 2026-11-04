//! The callback state the real engine graph runs, prepared and observed for lab tests.
use super::super::{LinuxProcessState, clock, failure::FailureMailbox};
use crate::backend::LinuxVirtualEndpointBackend;
use mixrelay_engine_core::session::AdapterRuntimeStatus;
use mixrelay_linux_pipewire::TransferSnapshot;
use mixrelay_render::{BlockProcessor, ProcessorExchange};
use std::sync::Arc;

impl<'a> LinuxProcessState<'a> {
    /// The state `run_audio_worker` builds, with `backend`'s pending owners already active.
    pub(crate) fn prepared(
        backend: &'a LinuxVirtualEndpointBackend,
        processor: Box<BlockProcessor>,
        exchange: &Arc<ProcessorExchange>,
        status: &Arc<AdapterRuntimeStatus>,
    ) -> Self {
        let format = processor.plan().format();
        let mut endpoints = Box::default();
        let mut captures = Box::default();
        let mut devices = Box::default();
        backend
            .activate_pending(
                processor.plan().revision(),
                &mut endpoints,
                &mut captures,
                &mut devices,
            )
            .expect("activate exact source and sink streams");
        backend.reclaim_retired();
        let (application_mailbox, device_mailbox) = backend.recovery_mailboxes();
        let application_recovery = application_mailbox
            .try_consumer()
            .expect("unique application capture recovery consumer");
        let device_recovery = device_mailbox
            .try_consumer()
            .expect("unique device recovery consumer");
        Self {
            failure: Arc::new(FailureMailbox::default()),
            resource_failures: Arc::new(FailureMailbox::default()),
            processor,
            exchange: Arc::clone(exchange),
            endpoint_backend: backend,
            application_recovery,
            device_recovery,
            endpoints,
            captures,
            devices,
            status: Arc::clone(status),
            timeline: clock::GraphTimeline::new(format.sample_rate).expect("valid graph timeline"),
            clock_diagnostics: Arc::default(),
        }
    }

    /// Transfer snapshots of the active capture, endpoint and device owners, in that order.
    pub(crate) fn transfer_snapshots(&self) -> [TransferSnapshot; 3] {
        [
            self.captures.transfer_snapshot(),
            self.endpoints.transfer_snapshot(),
            self.devices.transfer_snapshot(),
        ]
    }

    pub(crate) fn clock_diagnostics(&self) -> &Arc<clock::DiscontinuityMailbox> {
        &self.clock_diagnostics
    }
}
