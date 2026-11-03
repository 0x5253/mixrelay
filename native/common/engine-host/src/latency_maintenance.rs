use std::{
    sync::{Arc, Weak, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use mixrelay_render::{ProcessorExchange, RenderPlan};

/// One joined observer per engine, independent of editor snapshots and native configuration.
/// Declared before the audio owner so shutdown drains callbacks before stopping reclamation.
pub struct Observer {
    stop: mpsc::SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

impl Observer {
    /// Watches the exact initial plan and future successfully submitted plans.
    /// # Errors
    /// Returns a diagnostic if the single bounded background worker cannot be started.
    pub fn start(
        exchange: &Arc<ProcessorExchange>,
        plan: &Arc<RenderPlan>,
    ) -> Result<Self, String> {
        exchange.observe_plan(Arc::clone(plan));
        let (stop, receive) = mpsc::sync_channel(1);
        let exchange = Arc::downgrade(exchange);
        let worker = thread::Builder::new()
            .name("mixrelay-route-latency".into())
            .spawn(move || observe(&exchange, &receive))
            .map_err(|error| format!("could not start route latency observer: {error}"))?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

fn observe(exchange: &Weak<ProcessorExchange>, stop: &mpsc::Receiver<()>) {
    let mut failure = None;
    while matches!(
        stop.recv_timeout(Duration::from_millis(20)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ) {
        let Some(exchange) = exchange.upgrade() else {
            break;
        };
        // Cached atomics only. Native plug-ins, state files and editor locks are not consulted.
        match exchange.refresh_latency() {
            Ok(_) => failure = None,
            Err(error) => {
                if failure.as_ref() != Some(&error) {
                    tracing::error!(%error, "route latency preparation failed");
                }
                failure = Some(error);
            }
        }
    }
}

impl Observer {
    /// Stops and joins the observer.
    /// # Errors
    /// Reports a panicked observer.
    pub fn finish(mut self) -> Result<(), String> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<(), String> {
        let _ = self.stop.try_send(());
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| "route latency observer panicked".to_owned())
        })
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        if self.stop_and_join().is_err() {
            tracing::error!("route latency observer panicked");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_graph::{
        CompileContext, CompileLimits, GraphCompiler, GraphDocument, RenderFormat,
    };

    #[test]
    fn finishing_an_observer_joins_it() {
        let execution = GraphCompiler::default()
            .compile(
                &GraphDocument::new(1, Vec::new(), Vec::new()),
                CompileContext {
                    format: RenderFormat {
                        sample_rate: 48_000,
                        block_frames: 32,
                    },
                    limits: CompileLimits::default(),
                },
            )
            .unwrap();
        let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
        let exchange = Arc::new(ProcessorExchange::new(1));
        assert_eq!(Observer::start(&exchange, &plan).unwrap().finish(), Ok(()));
    }

    #[test]
    fn observer_publishes_without_ui_and_joins_on_drop() {
        let execution = GraphCompiler::default()
            .compile(
                &GraphDocument::new(1, Vec::new(), Vec::new()),
                CompileContext {
                    format: RenderFormat {
                        sample_rate: 48_000,
                        block_frames: 32,
                    },
                    limits: CompileLimits::default(),
                },
            )
            .unwrap();
        let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
        let exchange = Arc::new(ProcessorExchange::new(1));
        let observer = Observer::start(&exchange, &plan).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !exchange.has_pending_latency() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(exchange.has_pending_latency());
        drop(observer);
        assert_eq!(Arc::strong_count(&exchange), 1);
    }

    #[test]
    fn disconnected_stop_channel_terminates_without_an_owner() {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(sender);
        let worker = thread::spawn(move || observe(&Weak::new(), &receiver));
        assert!(worker.join().is_ok());
    }
}
