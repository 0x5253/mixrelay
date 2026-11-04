//! A dedicated control worker keeps native callbacks independent of engine latency.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use mixrelay_desktop_core::midi_commands;
use mixrelay_midi_control::{
    Binding, Target,
    learn::LearnCapture,
    mailbox::InputSequence,
    router::{Router, Source},
};
use mixrelay_time::{Deadline, Instant, MonotonicClock, Sleeper};

use super::{
    State,
    admission::Admission,
    lock,
    native::{self, NativeError, Port},
};
use crate::control_proxy::local::LocalControl;

/// Ports are rescanned and node names refreshed this often, and after every gain write.
const SCAN_INTERVAL_MS: u64 = 1000;
/// Native callbacks publish to atomics throughout this wait and all dispatch IO.
const TICK: Duration = Duration::from_millis(5);

/// Opens and closes native input ports as devices come and go.
pub(super) trait Inputs {
    type Connection;

    fn reconcile(
        &mut self,
        ports: &mut BTreeMap<String, Port<Self::Connection>>,
        capture: &Arc<LearnCapture>,
        sequence: &Arc<InputSequence>,
    ) -> Result<(), NativeError>;
}

/// `CoreMIDI` or ALSA sequencer inputs.
#[derive(Default)]
pub(super) struct NativeInputs {
    next_token: u32,
}

impl Inputs for NativeInputs {
    type Connection = midir::MidiInputConnection<Arc<mixrelay_midi_control::mailbox::Mailbox>>;

    fn reconcile(
        &mut self,
        ports: &mut BTreeMap<String, Port<Self::Connection>>,
        capture: &Arc<LearnCapture>,
        sequence: &Arc<InputSequence>,
    ) -> Result<(), NativeError> {
        native::reconcile(ports, capture, &mut self.next_token, sequence)
    }
}

/// The engine the worker reads node state from and sends mapped actions to.
pub(super) trait Engine {
    fn snapshot(&self) -> Option<serde_json::Value>;

    /// Sends one action, or a gain with the level it was computed from. Returns whether
    /// the engine applied it.
    fn act(
        &self,
        binding: &Binding,
        gain: Option<(f32, Option<f32>)>,
        admission: &Admission<'_>,
    ) -> bool;
}

impl Engine for LocalControl {
    fn snapshot(&self) -> Option<serde_json::Value> {
        tauri::async_runtime::block_on(self.midi_snapshot())
    }

    fn act(
        &self,
        binding: &Binding,
        gain: Option<(f32, Option<f32>)>,
        admission: &Admission<'_>,
    ) -> bool {
        tauri::async_runtime::block_on(self.midi_action(binding, gain, admission))
    }
}

/// State shared with the editor commands and shutdown.
#[derive(Clone, Copy)]
pub(super) struct Shared<'a> {
    pub(super) state: &'a Mutex<State>,
    pub(super) stop: &'a AtomicBool,
    pub(super) write_gate: &'a tokio::sync::Mutex<()>,
}

pub(super) struct Worker<'a, I: Inputs, E, C> {
    shared: Shared<'a>,
    inputs: I,
    engine: &'a E,
    clock: &'a C,
    start: Instant,
    ports: BTreeMap<String, Port<I::Connection>>,
    capture: Arc<LearnCapture>,
    sequence: Arc<InputSequence>,
    router: Router,
    /// The binding generation the router holds; `u64::MAX` forces a reload.
    generation: u64,
    next_scan_ms: u64,
    snapshot: serde_json::Value,
}

impl<'a, I, E, C> Worker<'a, I, E, C>
where
    I: Inputs,
    E: Engine,
    C: MonotonicClock + Sleeper,
{
    pub(super) fn new(shared: Shared<'a>, inputs: I, engine: &'a E, clock: &'a C) -> Self {
        let capture = Arc::clone(&lock(shared.state).capture);
        Self {
            shared,
            inputs,
            engine,
            clock,
            start: clock.now(),
            ports: BTreeMap::new(),
            capture,
            sequence: Arc::default(),
            router: Router::default(),
            generation: u64::MAX,
            next_scan_ms: 0,
            snapshot: serde_json::Value::Null,
        }
    }

    /// Ticks until shutdown is requested.
    pub(super) fn run(&mut self) {
        while !self.shared.stop.load(Ordering::Acquire) {
            self.tick();
            self.clock
                .sleep_until(Deadline::after(self.clock.now(), TICK));
        }
    }

    fn tick(&mut self) {
        let now = elapsed_ms(self.clock, self.start);
        if now >= self.next_scan_ms {
            self.scan();
            self.next_scan_ms = now.saturating_add(SCAN_INTERVAL_MS);
        }
        if self.reload_settings() {
            // Map mode: inputs belong to learn capture, not to bindings.
            let _ = lock(self.shared.state).observe_learn();
            return;
        }
        self.dispatch_inputs();
        if self.publish_gains(now) {
            self.next_scan_ms = 0;
        }
    }

    /// Reconciles ports, then refreshes the engine snapshot and remembered node names.
    fn scan(&mut self) {
        if self.refresh_ports() {
            // A reconnected port starts a fresh generation: faders pick up again.
            self.generation = u64::MAX;
        }
        self.snapshot = self.engine.snapshot().unwrap_or(serde_json::Value::Null);
        let mut state = lock(self.shared.state);
        if let Err(error) = state.refresh_names(&self.snapshot) {
            state.unavailable = Some(error.to_string());
        }
        state.snapshot = self.snapshot.clone();
    }

    /// Publishes the open ports to learn and status. Returns whether any port changed.
    fn refresh_ports(&mut self) -> bool {
        let previous = self.port_identities();
        let unavailable = self
            .inputs
            .reconcile(&mut self.ports, &self.capture, &self.sequence)
            .err();
        let changed = previous != self.port_identities();
        let mut state = lock(self.shared.state);
        state.mailboxes = self
            .ports
            .values()
            .map(|port| Arc::clone(&port.mailbox))
            .collect();
        state.ports = self
            .ports
            .values()
            .map(|port| (port.token, port.device.clone()))
            .collect();
        state.devices = self
            .ports
            .values()
            .map(|port| port.device.clone())
            .collect();
        state.unavailable = state
            .preferences_error
            .clone()
            .or_else(|| unavailable.map(|error| error.to_string()));
        changed
    }

    fn port_identities(&self) -> Vec<(String, u32)> {
        self.ports
            .values()
            .map(|port| (port.device.id.clone(), port.token))
            .collect()
    }

    /// Observes learn progress and loads changed bindings. Returns whether map mode is on.
    fn reload_settings(&mut self) -> bool {
        let mut state = lock(self.shared.state);
        if let Err(error) = state.observe_learn() {
            state.unavailable = Some(error.to_string());
        }
        if self.generation != state.store.generation() {
            self.generation = state.store.generation();
            self.router.load(state.store.bindings());
        }
        state.map_mode
    }

    fn admission(&self) -> Admission<'a> {
        Admission::new(
            self.shared.state,
            self.shared.stop,
            self.shared.write_gate,
            self.generation,
        )
    }

    fn dispatch_inputs(&mut self) {
        let admission = self.admission();
        let (state, generation) = (self.shared.state, self.generation);
        let engine = self.engine;
        let snapshot = &self.snapshot;
        self.router.drain(self.ports.values().map(|port| Source {
            device_id: &port.device.id,
            mailbox: &port.mailbox,
        }));
        self.router.dispatch(
            |target| ready_gain(snapshot, target),
            || admitted(state, generation),
            |binding| {
                engine.act(binding, None, &admission);
            },
        );
    }

    /// Returns whether a gain was attempted.
    fn publish_gains(&mut self, now: u64) -> bool {
        let admission = self.admission();
        let (state, generation) = (self.shared.state, self.generation);
        let engine = self.engine;
        let snapshot = &self.snapshot;
        let (clock, start) = (self.clock, self.start);
        self.router.publish_gains(now, |binding, value| {
            if !admitted(state, generation) {
                return None;
            }
            let observed = observed_gain(snapshot, &binding.target);
            let applied = engine.act(binding, Some((value, observed)), &admission);
            Some((applied, elapsed_ms(clock, start)))
        })
    }
}

fn elapsed_ms(clock: &impl MonotonicClock, start: Instant) -> u64 {
    u64::try_from(clock.now().duration_since(start).as_millis()).unwrap_or(u64::MAX)
}

/// Bindings still match the router's generation and the editor is not mapping.
fn admitted(state: &Mutex<State>, generation: u64) -> bool {
    let state = lock(state);
    state.store.generation() == generation && !state.map_mode
}

fn ready_gain(snapshot: &serde_json::Value, target: &Target) -> Option<f32> {
    if !midi_commands::volume_ready(snapshot, target) {
        return None;
    }
    observed_gain(snapshot, target)
}

fn observed_gain(snapshot: &serde_json::Value, target: &Target) -> Option<f32> {
    midi_commands::node(snapshot, target)
        .and_then(|node| serde_json::from_value(node["gainDb"].clone()).ok())
}

#[cfg(test)]
mod tests;
