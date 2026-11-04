//! Control-side supervision of an installed direct renderer: it watches the
//! renderer's signals and lifetime requests, and never moves PCM itself.
use super::{DirectPlan, renderer::RenderSignals};
use crate::macos::AudioContext;
use std::{
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

const EXECUTION_REPORT_INTERVAL: Duration = Duration::from_secs(5);
const STALL_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn log_route(plan: &DirectPlan, rate: u32, buffer: u32) {
    tracing::info!(
        clock_id = plan.clock_id,
        processing_rate = rate,
        buffer_frames = buffer,
        bindings = plan.bindings.len(),
        "CoreAudio direct renderer starting"
    );
}

impl RenderSignals {
    /// Waits until a stop, pause, rebind, device change, failure or stalled
    /// callback ends this direct lifetime.
    pub(super) fn supervise(
        &self,
        plan: &DirectPlan,
        context: AudioContext<'_>,
        last_execution_report: &mut Instant,
    ) {
        let mut ticks = 0;
        let mut last = Instant::now();
        while !context.runtime_status.stop_requested()
            && !context.runtime_status.pause_requested()
            && !self.rebind.load(Ordering::Acquire)
            && !context.devices.has_pending()
            && plan.healthy()
        {
            if self.failed.load(Ordering::Acquire) || self.invalid.load(Ordering::Acquire) {
                break;
            }
            if last_execution_report.elapsed() >= EXECUTION_REPORT_INTERVAL {
                plan.report_execution(last_execution_report.elapsed());
                *last_execution_report = Instant::now();
            }
            let next = self.progress.load(Ordering::Acquire);
            if next != ticks {
                ticks = next;
                last = Instant::now();
            } else if last.elapsed() > STALL_TIMEOUT {
                tracing::warn!(
                    clock_id = plan.clock_id,
                    "CoreAudio direct callback progress stalled"
                );
                break;
            }
            // Supervisory responsiveness only. This thread never moves PCM; every
            // block has already been rendered synchronously inside the IOProc.
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Requests reconnection or recovery for what ended the lifetime.
    pub(super) fn finish(
        &self,
        plan: &DirectPlan,
        context: AudioContext<'_>,
    ) -> Result<(), String> {
        let invalid = self.invalid.load(Ordering::Acquire);
        let failed = self.failed.load(Ordering::Acquire);
        tracing::info!(
            clock_id = plan.clock_id,
            invalid_layout = invalid,
            render_failed = failed,
            stop = context.runtime_status.stop_requested(),
            pause = context.runtime_status.pause_requested(),
            graph_pending = context.exchange.has_pending(),
            devices_pending = context.devices.has_pending(),
            healthy = plan.healthy(),
            "CoreAudio direct renderer stopped"
        );
        if invalid {
            tracing::warn!(
                clock_id = plan.clock_id,
                "CoreAudio callback rejected native buffer layout"
            );
            plan.request_reconnect();
        }
        if !plan.healthy() {
            context.devices.request_recovery();
        }
        if failed {
            return Err(
                "CoreAudio direct callback rejected buffer geometry or DSP processing".into(),
            );
        }
        Ok(())
    }
}
