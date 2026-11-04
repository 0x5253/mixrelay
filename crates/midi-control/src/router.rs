//! Routes drained controller input to bindings: reception order, fan-out, pick-up and
//! bounded-rate gain publication.
use crate::{
    Action, Binding, Mode, Target,
    mailbox::{Input, Mailbox, reception_order},
    runtime::{Coalescer, Pickup},
};

/// One open input and the stable device ID its bindings name.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub device_id: &'a str,
    pub mailbox: &'a Mailbox,
}

/// Input state for one mapping generation.
#[derive(Debug, Default)]
pub struct Router {
    bindings: Vec<Binding>,
    pickup: Vec<Pickup>,
    gains: Coalescer,
    inputs: Vec<(usize, Input)>,
}

impl Router {
    /// Starts a new mapping generation: every fader must pick up again and pending gains
    /// from the previous mapping are dropped.
    pub fn load(&mut self, bindings: &[Binding]) {
        self.bindings.clear();
        self.bindings.extend_from_slice(bindings);
        self.pickup.clear();
        self.pickup.resize_with(bindings.len(), Pickup::default);
        self.gains.clear();
        self.inputs.clear();
    }

    #[must_use]
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Takes every bound input that arrived before a shared cross-port boundary, in
    /// reception order. Nothing is taken while a callback is still publishing.
    pub fn drain<'a, I>(&mut self, sources: I)
    where
        I: IntoIterator<Item = Source<'a>> + Clone,
    {
        self.drain_with(sources, |_| {});
    }

    fn drain_with<'a, I>(&mut self, sources: I, mut before_presses: impl FnMut(&str))
    where
        I: IntoIterator<Item = Source<'a>> + Clone,
    {
        self.inputs.clear();
        let Some(boundary) = sources
            .clone()
            .into_iter()
            .next()
            .map(|source| source.mailbox.boundary())
        else {
            return;
        };
        if sources
            .clone()
            .into_iter()
            .any(|source| source.mailbox.is_publishing())
        {
            return;
        }
        for source in sources {
            self.take_volumes(source);
            let presses = source.mailbox.take_presses_before(boundary);
            before_presses(source.device_id);
            for input in presses {
                self.fan_out_press(source.device_id, input);
            }
        }
        let bindings = &self.bindings;
        self.inputs.sort_by(|(left, a), (right, b)| {
            reception_order(a.sequence(&bindings[*left]), b.sequence(&bindings[*right]))
        });
    }

    /// Drains each hardware slot once, then fans the same batch out to every volume mapping.
    fn take_volumes(&mut self, source: Source<'_>) {
        for (index, binding) in self.bindings.iter().enumerate() {
            if binding.action != Action::Volume || binding.control.device.id != source.device_id {
                continue;
            }
            if self.bindings[..index].iter().any(|other| {
                other.action == Action::Volume && other.control.matches(&binding.control)
            }) {
                continue;
            }
            let control = &binding.control;
            if let Some(input) = source
                .mailbox
                .take(control.channel, control.kind, control.number)
            {
                for (other, mapping) in self.bindings.iter().enumerate() {
                    if mapping.action == Action::Volume && mapping.control.matches(control) {
                        self.inputs.push((other, input));
                    }
                }
            }
        }
    }

    fn fan_out_press(&mut self, device_id: &str, input: Input) {
        for (index, binding) in self.bindings.iter().enumerate() {
            let control = &binding.control;
            if binding.action != Action::Volume
                && control.device.id == device_id
                && control.channel == input.channel
                && control.kind == input.kind
                && control.number == input.number
            {
                self.inputs.push((index, input));
            }
        }
    }

    /// Applies drained input in reception order while `admitted` holds. Volume input moves
    /// pick-up and queues a gain; every other press is sent once and never replayed.
    ///
    /// `ready_gain` returns the observed gain of a target that accepts volume control now.
    /// Once `admitted` fails, the rest of the batch and every pending gain are dropped.
    pub fn dispatch(
        &mut self,
        ready_gain: impl Fn(&Target) -> Option<f32>,
        admitted: impl Fn() -> bool,
        mut send: impl FnMut(&Binding),
    ) {
        for &(index, input) in &self.inputs {
            if !admitted() {
                self.gains.clear();
                break;
            }
            let binding = &self.bindings[index];
            let pickup = &mut self.pickup[index];
            if binding.action == Action::Volume {
                queue_gain(binding, pickup, input, &mut self.gains, &ready_gain);
            } else {
                for _ in 0..input.presses {
                    if admitted() {
                        send(binding);
                    }
                }
            }
        }
        if !admitted() {
            self.gains.clear();
        }
    }

    /// Sends at most one due gain per volume target. `send` returns whether the gain was
    /// applied and when the attempt completed, or `None` to drop every pending gain.
    /// A rejected gain makes every fader on that target pick up again.
    ///
    /// Returns whether any gain was attempted.
    pub fn publish_gains(
        &mut self,
        now_ms: u64,
        mut send: impl FnMut(&Binding, f32) -> Option<(bool, u64)>,
    ) -> bool {
        let mut attempted = false;
        for binding in self
            .bindings
            .iter()
            .filter(|binding| binding.action == Action::Volume)
        {
            let Some(value) = self.gains.take(&binding.target, now_ms) else {
                continue;
            };
            let Some((applied, completed)) = send(binding, value) else {
                self.gains.clear();
                break;
            };
            self.gains.complete(&binding.target, completed);
            if !applied {
                for (other, pickup) in self.bindings.iter().zip(&mut self.pickup) {
                    if other.target == binding.target {
                        *pickup = Pickup::default();
                    }
                }
            }
            attempted = true;
        }
        attempted
    }
}

fn queue_gain(
    binding: &Binding,
    pickup: &mut Pickup,
    input: Input,
    gains: &mut Coalescer,
    ready_gain: impl Fn(&Target) -> Option<f32>,
) {
    let Some(observed) = ready_gain(&binding.target) else {
        *pickup = Pickup::default();
        return;
    };
    let current = if binding.mode == Mode::Absolute {
        observed
    } else {
        gains.desired(&binding.target).unwrap_or(observed)
    };
    if let Some(value) = pickup.map(binding, input, current) {
        gains.push(binding.target.clone(), value);
    }
}

#[cfg(test)]
mod tests;
