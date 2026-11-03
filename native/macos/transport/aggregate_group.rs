//! Immutable geometry and readback policy for an already planned private aggregate.
//! Admission and compatibility remain in the engine's group planner; this state
//! verifies that `CoreAudio` published the same identity, layout and clock policy.

mod native;
/// The aggregate entry points beneath a simulated HAL, for hermetic tests.
#[cfg(any(test, feature = "test-observation"))]
pub use native::test_observation;
pub use native::{
    AggregateGroup, NativeBinding, mr_aggregate_group_binding, mr_aggregate_group_create,
    mr_aggregate_group_ready, mr_aggregate_group_try_destroy, mr_aggregate_member_info,
};
#[cfg(test)]
mod tests;

const MAX_MEMBERS: usize = 32;

/// Native facts captured for a single physical member.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MemberInfo {
    device: u32,
    transport: u32,
    rate: f64,
    input: u32,
    output: u32,
    alive: u32,
}

/// Immutable channel geometry retained by its native aggregate owner.
#[derive(Debug)]
struct AggregateGeometry {
    members: [MemberGeometry; MAX_MEMBERS],
    count: u32,
    primary: u32,
    rate: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct MemberGeometry {
    device: u32,
    input: u32,
    output: u32,
}

/// `CoreAudio` composition facts projected without deciding whether they are ready.
#[repr(C)]
#[derive(Clone, Debug, Default)]
struct Readback {
    active: [u32; MAX_MEMBERS],
    drift: [i32; MAX_MEMBERS],
    ordered_uids_match: [u8; MAX_MEMBERS],
    composition_uids_match: [u8; MAX_MEMBERS],
    primary_uid_matches: u32,
    input: u32,
    output: u32,
}

/// A member's contiguous channel projection inside the aggregate.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Binding {
    offset: u32,
    total: u32,
}

#[derive(Default)]
struct ChannelTotals {
    input: u32,
    output: u32,
}

impl AggregateGeometry {
    fn new(members: &[MemberInfo], primary: u32) -> Option<Self> {
        if !(2..=MAX_MEMBERS).contains(&members.len()) || primary as usize >= members.len() {
            return None;
        }
        let mut state = Self {
            members: [MemberGeometry::default(); MAX_MEMBERS],
            count: u32::try_from(members.len()).ok()?,
            primary,
            rate: members[primary as usize].rate,
        };
        for (stored, info) in state.members.iter_mut().zip(members) {
            *stored = MemberGeometry {
                device: info.device,
                input: info.input,
                output: info.output,
            };
        }
        state.totals()?;
        Some(state)
    }

    fn members(&self) -> &[MemberGeometry] {
        &self.members[..self.count as usize]
    }

    fn totals(&self) -> Option<ChannelTotals> {
        self.members()
            .iter()
            .try_fold(ChannelTotals::default(), |sum, member| {
                Some(ChannelTotals {
                    input: sum.input.checked_add(member.input)?,
                    output: sum.output.checked_add(member.output)?,
                })
            })
    }

    fn member_matches(&self, index: u32, observed: &MemberInfo) -> bool {
        self.members().get(index as usize).is_some_and(|expected| {
            observed.device != 0
                && observed.device == expected.device
                && observed.alive != 0
                // The planner admits positive integral nominal rates, not noisy
                // clock measurements. A changed rate must never pass an epsilon test.
                && observed.rate.to_bits() == self.rate.to_bits()
                && observed.input == expected.input
                && observed.output == expected.output
        })
    }

    fn ready(&self, readback: &Readback) -> bool {
        let Some(totals) = self.totals() else {
            return false;
        };
        let active = &readback.active[..self.count as usize];
        readback.primary_uid_matches != 0
            && readback.input == totals.input
            && readback.output == totals.output
            && self.members().iter().enumerate().all(|(index, member)| {
                readback.ordered_uids_match[index] != 0
                    && readback.composition_uids_match[index] != 0
                    && (readback.drift[index] != 0) == (index != self.primary as usize)
                    && active.contains(&member.device)
            })
    }

    fn binding(&self, index: u32, input: bool, channels: u32) -> Option<Binding> {
        let index = index as usize;
        let width = |member: &MemberGeometry| if input { member.input } else { member.output };
        let member = self.members().get(index)?;
        if channels == 0 || channels > width(member) {
            return None;
        }
        Some(Binding {
            offset: self.members()[..index].iter().map(width).sum(),
            total: self.members().iter().map(width).sum(),
        })
    }
}

const _: () = assert!(size_of::<MemberInfo>() == 32);
const _: () = assert!(size_of::<AggregateGeometry>() == 400);
const _: () = assert!(align_of::<AggregateGeometry>() == 8);
const _: () = assert!(size_of::<Readback>() == 332);
const _: () = assert!(size_of::<Binding>() == 8);
