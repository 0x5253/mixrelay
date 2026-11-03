//! One coalescing control worker prepares inventory without blocking HAL grant delivery.
use super::virtual_pcm::{PendingGrants, ThreadWorker, VirtualPcm, Worker};
use mixrelay_macos_transport::client_pcm::EndpointGeometry;
use std::{sync::Arc, thread};

pub(super) struct Job {
    pub(super) geometry: Vec<EndpointGeometry>,
    pub(super) pending: PendingGrants,
    pub(super) observed: u64,
    pub(super) received: u64,
}
#[derive(Default)]
struct State {
    pending: Option<Job>,
    stopped: bool,
    #[cfg(test)]
    busy: bool,
}
#[derive(Default)]
struct Shared {
    state: parking_lot::Mutex<State>,
    changed: parking_lot::Condvar,
}
pub(super) struct Inventory<W: Worker = ThreadWorker> {
    shared: Arc<Shared>,
    worker: parking_lot::Mutex<Option<thread::JoinHandle<()>>>,
    marker: std::marker::PhantomData<fn(W)>,
}
impl<W: Worker> Inventory<W> {
    pub(super) fn new() -> Self {
        Self {
            shared: Arc::default(),
            worker: parking_lot::Mutex::new(None),
            marker: std::marker::PhantomData,
        }
    }
    pub(super) fn stopped(&self) -> bool {
        self.shared.state.lock().stopped
    }
    pub(super) fn submit(&self, pcm: Arc<VirtualPcm<W>>, job: Job) -> std::io::Result<()> {
        let mut worker = self.worker.lock();
        let mut state = self.shared.state.lock();
        if state.stopped {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        if worker.is_none() {
            let shared = Arc::clone(&self.shared);
            *worker = Some(
                thread::Builder::new()
                    .name("mixrelay-input-prepare".into())
                    .spawn(move || run(&pcm, &shared))?,
            );
        }
        if state
            .pending
            .as_ref()
            .is_none_or(|pending| pending.observed <= job.observed)
        {
            state.pending = Some(job);
        }
        self.shared.changed.notify_one();
        Ok(())
    }
    pub(super) fn stop(&self) {
        let mut worker = self.worker.lock();
        let mut state = self.shared.state.lock();
        state.stopped = true;
        let pending = state.pending.take();
        self.shared.changed.notify_one();
        drop(state);
        drop(pending);
        if let Some(worker) = worker.take() {
            let _ = mixrelay_engine_runtime::join_worker(worker);
        }
    }
}
impl<W: Worker> Drop for Inventory<W> {
    fn drop(&mut self) {
        self.stop();
    }
}
fn run<W: Worker>(pcm: &VirtualPcm<W>, shared: &Shared) {
    // Reservation retries share the converter policy: 10 ms through one second.
    let mut retry = std::time::Duration::from_millis(10);
    loop {
        let job = {
            let mut state = shared.state.lock();
            while state.pending.is_none() && !state.stopped {
                shared.changed.wait(&mut state);
            }
            if state.stopped {
                return;
            }
            #[cfg(test)]
            {
                state.busy = true;
            }
            state.pending.take()
        };
        if let Some(mut job) = job {
            match prepare(pcm, &mut job) {
                Ok(()) => retry = std::time::Duration::from_millis(10),
                Err(error) if error.retryable() => {
                    tracing::warn!(boundary = "hal_shared_pcm", stage = "inventory_prepare", %error, retry_ms = retry.as_millis(), "virtual input inventory preparation deferred");
                    let mut state = shared.state.lock();
                    if state.pending.is_none() && !state.stopped {
                        shared.changed.wait_for(&mut state, retry);
                        if state.pending.is_none() && !state.stopped {
                            state.pending = Some(job);
                        }
                    }
                    retry = (retry * 2).min(std::time::Duration::from_secs(1));
                }
                // A clear or newer geometry invalidates a pending snapshot.
                Err(super::virtual_pcm::Error::Stale) => {}
                Err(error) => {
                    tracing::error!(boundary = "hal_shared_pcm", stage = "inventory_prepare", %error, "virtual input inventory preparation failed");
                }
            }
        }
        #[cfg(test)]
        {
            let mut state = shared.state.lock();
            state.busy = false;
            shared.changed.notify_all();
        }
    }
}
fn prepare<W: Worker>(pcm: &VirtualPcm<W>, job: &mut Job) -> Result<(), super::virtual_pcm::Error> {
    let configured =
        pcm.configure_pending(&job.geometry, &mut job.pending, job.observed, job.received);
    pcm.replace_pending(&mut job.pending, job.observed, job.received)?;
    configured
}

#[cfg(test)]
pub(super) fn prepare_for_test<W: Worker>(
    pcm: &VirtualPcm<W>,
    job: &mut Job,
) -> Result<(), super::virtual_pcm::Error> {
    prepare(pcm, job)
}

#[cfg(test)]
mod tests;
