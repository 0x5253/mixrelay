//! A dedicated control worker keeps native callbacks independent of engine latency.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use mixrelay_midi_control::{
    Action, Binding, Mode,
    mailbox::{Input, InputSequence, reception_order},
    runtime::{Coalescer, Pickup},
};

use super::{
    State, lock,
    native::{self, Port},
};
use crate::control_proxy::local::LocalControl;

pub(super) fn run(
    state: &Mutex<State>,
    stop: &AtomicBool,
    control: &Arc<LocalControl>,
    write_gate: &tokio::sync::Mutex<()>,
    clock: &(impl mixrelay_time::MonotonicClock + mixrelay_time::Sleeper),
) {
    let mut ports = BTreeMap::new();
    let capture = Arc::clone(&lock(state).capture);
    let mut next_token = 0;
    let sequence = Arc::default();
    let mut gains = Coalescer::default();
    let mut pickup = Vec::new();
    let mut bindings = Vec::new();
    let mut inputs = Vec::new();
    let mut generation = u64::MAX;
    let start = clock.now();
    let mut scan = 0;
    let mut snapshot = serde_json::Value::Null;
    while !stop.load(Ordering::Acquire) {
        let now = u64::try_from(clock.now().duration_since(start).as_millis()).unwrap_or(u64::MAX);
        if now >= scan {
            if refresh_ports(state, &mut ports, &capture, &mut next_token, &sequence) {
                generation = u64::MAX;
            }
            snapshot = tauri::async_runtime::block_on(control.midi_snapshot())
                .unwrap_or(serde_json::Value::Null);
            {
                let mut state = lock(state);
                if let Err(error) = state.refresh_names(&snapshot) {
                    state.unavailable = Some(error.to_string());
                }
                state.snapshot = snapshot.clone();
            }
            scan = now.saturating_add(1000);
        }
        let learning = refresh_settings(
            state,
            &mut generation,
            &mut bindings,
            &mut pickup,
            &mut gains,
        );
        let admission = super::admission::Admission::new(state, stop, write_gate, generation);
        if learning {
            let _ = lock(state).observe_learn();
        } else {
            drain_inputs(&ports, &bindings, &mut inputs);
            process_inputs(
                (&bindings, &mut pickup),
                &inputs,
                (&mut gains, &snapshot),
                (state, generation),
                |binding| {
                    tauri::async_runtime::block_on(control.midi_action(binding, None, &admission));
                },
            );
            if publish_gains(&bindings, &mut pickup, &mut gains, now, |binding, value| {
                if !admitted(state, generation) {
                    return None;
                }
                let applied = tauri::async_runtime::block_on(control.midi_action(
                    binding,
                    Some((value, observed_gain(&snapshot, &binding.target))),
                    &admission,
                ));
                Some((
                    applied,
                    u64::try_from(clock.now().duration_since(start).as_millis())
                        .unwrap_or(u64::MAX),
                ))
            }) {
                scan = 0;
            }
        }
        // Native callbacks publish to atomics throughout this wait and all dispatch IO.
        clock.sleep_until(mixrelay_time::Deadline::after(
            clock.now(),
            Duration::from_millis(5),
        ));
    }
}

fn publish_gains(
    bindings: &[Binding],
    pickup: &mut [Pickup],
    gains: &mut Coalescer,
    now: u64,
    mut send: impl FnMut(&Binding, f32) -> Option<(bool, u64)>,
) -> bool {
    let mut attempted = false;
    for binding in bindings
        .iter()
        .filter(|binding| binding.action == Action::Volume)
    {
        if let Some(value) = gains.take(&binding.target, now) {
            let Some((applied, completed)) = send(binding, value) else {
                gains.clear();
                break;
            };
            gains.complete(&binding.target, completed);
            if !applied {
                for (other, pickup) in bindings.iter().zip(&mut *pickup) {
                    if other.target == binding.target {
                        *pickup = Pickup::default();
                    }
                }
            }
            attempted = true;
        }
    }
    attempted
}

fn refresh_ports(
    state: &Mutex<State>,
    ports: &mut BTreeMap<String, Port>,
    capture: &Arc<mixrelay_midi_control::learn::LearnCapture>,
    next_token: &mut u32,
    sequence: &Arc<InputSequence>,
) -> bool {
    let previous: Vec<_> = ports
        .values()
        .map(|port| (port.device.id.clone(), port.token))
        .collect();
    let unavailable = native::reconcile(ports, capture, next_token, sequence).err();
    let changed = previous
        != ports
            .values()
            .map(|port| (port.device.id.clone(), port.token))
            .collect::<Vec<_>>();
    let mut state = lock(state);
    state.mailboxes = ports
        .values()
        .map(|port| Arc::clone(&port.mailbox))
        .collect();
    state.ports = ports
        .values()
        .map(|port| (port.token, port.device.clone()))
        .collect();
    state.devices = ports.values().map(|port| port.device.clone()).collect();
    state.unavailable = state
        .preferences_error
        .clone()
        .or_else(|| unavailable.map(|error| error.to_string()));
    changed
}

fn refresh_settings(
    state: &Mutex<State>,
    generation: &mut u64,
    bindings: &mut Vec<Binding>,
    pickup: &mut Vec<Pickup>,
    gains: &mut Coalescer,
) -> bool {
    let mut state = lock(state);
    if let Err(error) = state.observe_learn() {
        state.unavailable = Some(error.to_string());
    }
    if *generation != state.store.generation() {
        *generation = state.store.generation();
        bindings.clear();
        bindings.extend_from_slice(state.store.bindings());
        pickup.clear();
        pickup.extend((0..bindings.len()).map(|_| Pickup::default()));
        gains.clear();
    }
    state.map_mode
}

fn drain_inputs<C>(
    ports: &BTreeMap<String, Port<C>>,
    bindings: &[Binding],
    inputs: &mut Vec<(usize, Input)>,
) {
    drain_inputs_with(ports, bindings, inputs, |_| {});
}

fn drain_inputs_with<C>(
    ports: &BTreeMap<String, Port<C>>,
    bindings: &[Binding],
    inputs: &mut Vec<(usize, Input)>,
    mut before_presses: impl FnMut(&str),
) {
    inputs.clear();
    let Some(boundary) = ports.values().next().map(|port| port.mailbox.boundary()) else {
        return;
    };
    if ports.values().any(|port| port.mailbox.is_publishing()) {
        return;
    }
    for port in ports.values() {
        for (index, binding) in bindings.iter().enumerate() {
            if binding.action != Action::Volume || binding.control.device.id != port.device.id {
                continue;
            }
            // Drain each hardware slot once, then fan out the same batch to every volume mapping.
            if bindings[..index].iter().any(|other| {
                other.action == Action::Volume && other.control.matches(&binding.control)
            }) {
                continue;
            }
            if let Some(input) = port.mailbox.take(
                binding.control.channel,
                binding.control.kind,
                binding.control.number,
            ) {
                for (other, mapping) in bindings.iter().enumerate() {
                    if mapping.action == Action::Volume && mapping.control.matches(&binding.control)
                    {
                        inputs.push((other, input));
                    }
                }
            }
        }
        let presses = port.mailbox.take_presses_before(boundary);
        before_presses(&port.device.id);
        for input in presses {
            for (index, binding) in bindings.iter().enumerate() {
                if binding.action != Action::Volume
                    && binding.control.device.id == port.device.id
                    && binding.control.channel == input.channel
                    && binding.control.kind == input.kind
                    && binding.control.number == input.number
                {
                    inputs.push((index, input));
                }
            }
        }
    }
    inputs.sort_by(|(left, a), (right, b)| {
        reception_order(a.sequence(&bindings[*left]), b.sequence(&bindings[*right]))
    });
}

fn process_inputs(
    binding_states: (&[Binding], &mut [Pickup]),
    inputs: &[(usize, Input)],
    gain_state: (&mut Coalescer, &serde_json::Value),
    admission: (&Mutex<State>, u64),
    mut send: impl FnMut(&Binding),
) {
    let (bindings, pickup) = binding_states;
    let (gains, snapshot) = gain_state;
    for (index, input) in inputs {
        if !admitted(admission.0, admission.1) {
            gains.clear();
            break;
        }
        apply(
            (&bindings[*index], &mut pickup[*index]),
            *input,
            gains,
            snapshot,
            |binding| {
                if admitted(admission.0, admission.1) {
                    send(binding);
                }
            },
        );
    }
    if !admitted(admission.0, admission.1) {
        gains.clear();
    }
}

fn admitted(state: &Mutex<State>, generation: u64) -> bool {
    let state = lock(state);
    state.store.generation() == generation && !state.map_mode
}

fn apply(
    binding_state: (&Binding, &mut Pickup),
    input: Input,
    gains: &mut Coalescer,
    snapshot: &serde_json::Value,
    mut send: impl FnMut(&Binding),
) {
    let (binding, pickup) = binding_state;
    if binding.action == Action::Volume {
        if !mixrelay_desktop_core::midi_commands::volume_ready(snapshot, &binding.target) {
            *pickup = Pickup::default();
            return;
        }
        let observed = mixrelay_desktop_core::midi_commands::node(snapshot, &binding.target)
            .and_then(|node| serde_json::from_value::<f32>(node["gainDb"].clone()).ok());
        let current = if binding.mode == Mode::Absolute {
            observed
        } else {
            gains.desired(&binding.target).or(observed)
        };
        if let Some(current) = current
            && let Some(value) = pickup.map(binding, input, current)
        {
            gains.push(binding.target.clone(), value);
        }
    } else {
        // Dispatch each captured press once; uncertain writes are never replayed.
        for _ in 0..input.presses {
            send(binding);
        }
    }
}

fn observed_gain(
    snapshot: &serde_json::Value,
    target: &mixrelay_midi_control::Target,
) -> Option<f32> {
    mixrelay_desktop_core::midi_commands::node(snapshot, target)
        .and_then(|node| serde_json::from_value(node["gainDb"].clone()).ok())
}

#[cfg(test)]
mod tests;
