//! One shared publisher; subscriptions start with its complete current observation.
mod publication;

use publication::Publication;

use crate::{ControlBackend, ControlOperations, ControlState, METER_TELEMETRY_INTERVAL};
use axum::{
    extract::State,
    response::sse::{Event, Sse},
};
use futures_util::StreamExt as _;
use mixrelay_control_protocol::{ControlTelemetry, TelemetryEncoder};
use std::sync::{Arc, OnceLock};
use tokio::sync::{broadcast, watch};

/// Windows a subscriber may fall behind by before its stream ends (and it resubscribes).
const BACKLOG: usize = 64;

/// The current complete packet and the window broadcast. Receivers only, so both close
/// when the publisher ends.
type Channels = (
    watch::Receiver<Option<ControlTelemetry>>,
    broadcast::Receiver<ControlTelemetry>,
);

#[derive(Default)]
pub(super) struct Publisher {
    channels: OnceLock<Channels>,
}

/// One subscriber's view: the current complete packet, then every later window.
pub(super) struct Subscription {
    latest: watch::Receiver<Option<ControlTelemetry>>,
    windows: broadcast::Receiver<ControlTelemetry>,
    started: bool,
    sent: u64,
}

impl Publisher {
    fn subscribe(
        &self,
        backend: Arc<dyn ControlBackend>,
        operations: ControlOperations,
    ) -> Subscription {
        let (latest, windows) = self.channels.get_or_init(|| {
            let (sender, latest) = watch::channel(None);
            let (windows, receiver) = broadcast::channel(BACKLOG);
            tokio::spawn(publish(backend, operations, sender, windows));
            (latest, receiver)
        });
        // Subscribe to windows before reading the snapshot, so none falls between them.
        let windows = windows.resubscribe();
        Subscription {
            latest: latest.clone(),
            windows,
            started: false,
            sent: 0,
        }
    }
}

impl Subscription {
    /// The current snapshot first, then each later window. Windows that piled up while
    /// this client was slow fold into one (meters keep the interval maximum, state the
    /// latest), so backpressure never discards a peak. `None` once the publisher ends or
    /// this client fell [`BACKLOG`] windows behind.
    async fn next(&mut self) -> Option<ControlTelemetry> {
        let mut first = if self.started {
            None
        } else {
            self.started = true;
            self.latest.borrow().clone()
        };
        let mut packet = loop {
            let next = match first.take() {
                Some(packet) => packet,
                None => self.windows.recv().await.ok()?,
            };
            if next.sequence > self.sent {
                break next;
            }
        };
        loop {
            match self.windows.try_recv() {
                Ok(next) if next.sequence > packet.sequence => packet.absorb(next),
                Ok(_) => {}
                // Peaks were overwritten; resubscribing restarts from the snapshot.
                Err(broadcast::error::TryRecvError::Lagged(_)) => return None,
                // A closed publisher still delivers what it published.
                Err(_) => break,
            }
        }
        self.sent = packet.sequence;
        Some(packet)
    }
}

// Native editor closures, failures and recorder transitions happen outside HTTP mutations.
// Observe them on a blocking worker, once per shared publication window, never on audio
// or reactor threads. Only changed state is transmitted; the watch slot is a full snapshot.
async fn publish(
    backend: Arc<dyn ControlBackend>,
    operations: ControlOperations,
    sender: watch::Sender<Option<ControlTelemetry>>,
    windows: broadcast::Sender<ControlTelemetry>,
) {
    let mut interval = tokio::time::interval(METER_TELEMETRY_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut publication = Publication::default();
    loop {
        tokio::select! {
            () = operations.closed() => return,
            _ = interval.tick() => {},
        }
        let backend = Arc::clone(&backend);
        let Ok(packet) = tokio::task::spawn_blocking(move || observe(backend.as_ref())).await
        else {
            return;
        };
        let Some(packet) = publication.prepare(packet) else {
            continue;
        };
        sender.send_replace(Some(packet.clone()));
        // No subscriber is not an error; the next one starts from the snapshot.
        let _ = windows.send(packet);
    }
}

fn observe(backend: &dyn ControlBackend) -> ControlTelemetry {
    ControlTelemetry {
        engine_incarnation: backend.incarnation(),
        sequence: 0,
        meters: backend.meters(),
        state: backend.telemetry_state(),
    }
}

pub(super) async fn subscribe(
    State(state): State<ControlState>,
    axum::Extension(operations): axum::Extension<ControlOperations>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let subscription = state.telemetry.subscribe(state.backend, operations.clone());
    // Each stream has its own encoder: meter bindings follow only when they change.
    let stream = futures_util::stream::unfold(
        (subscription, None, TelemetryEncoder::default()),
        |(mut subscription, generation, mut encoder)| async move {
            let mut packet = subscription.next().await?;
            let next_generation = packet.state.as_ref().map(|state| state.generation);
            if generation == next_generation {
                packet.state = None;
            }
            let event = Event::default()
                .event("telemetry")
                .data(encoder.json(&packet).to_string());
            Some((Ok(event), (subscription, next_generation, encoder)))
        },
    )
    .take_until(operations.closed());
    Sse::new(stream)
}

#[cfg(test)]
mod tests;
