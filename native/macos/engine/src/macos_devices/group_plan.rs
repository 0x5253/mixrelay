//! Private-group compatibility policy. Native code supplies facts, not fallback
//! decisions. A rejected plan never allocates a `CoreAudio` aggregate.
use std::{
    collections::BTreeSet,
    ffi::{CStr, CString},
};

const MAX_MEMBERS: usize = 32;
const MAX_CHANNELS: u32 = 2048;
const AGGREGATE_TRANSPORT: u32 = u32::from_be_bytes(*b"grup");

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct MemberInfo {
    pub(super) device: u32,
    pub(super) transport: u32,
    pub(super) rate: f64,
    pub(super) input: u32,
    pub(super) output: u32,
    pub(super) alive: u32,
}

pub(super) struct GroupPlan {
    pub(super) names: Vec<CString>,
    pub(super) facts: Vec<MemberInfo>,
}

impl GroupPlan {
    /// Plans a private group from each member's current `snapshot`. None for any member set
    /// that must keep independent bridges; no group is created for a rejected plan.
    pub(super) fn prepare(
        members: &[String],
        mut snapshot: impl FnMut(&CStr) -> Option<MemberInfo>,
    ) -> Result<Option<Self>, String> {
        if !(2..=MAX_MEMBERS).contains(&members.len())
            || members.iter().any(String::is_empty)
            || members.iter().collect::<BTreeSet<_>>().len() != members.len()
        {
            return Ok(None);
        }
        let names = members
            .iter()
            .map(|uid| CString::new(uid.as_str()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let mut facts = Vec::<MemberInfo>::with_capacity(names.len());
        let mut devices = BTreeSet::new();
        let (mut inputs, mut outputs) = (0_u32, 0_u32);
        for uid in &names {
            let Some(info) = snapshot(uid.as_c_str()) else {
                return Ok(None);
            };
            if info.device == 0
                || !devices.insert(info.device)
                || info.alive != 1
                || info.transport == 0
                || info.transport == AGGREGATE_TRANSPORT
                || !info.rate.is_finite()
                || !(8000.0..=384_000.0).contains(&info.rate)
                || info.rate.fract() != 0.0
                // These are positive integral nominal rates, not noisy clock
                // measurements: an epsilon could incorrectly admit conversion.
                || facts.first().is_some_and(|first| first.rate.to_bits() != info.rate.to_bits())
                || info.input > MAX_CHANNELS - inputs
                || info.output > MAX_CHANNELS - outputs
            {
                return Ok(None);
            }
            inputs += info.input;
            outputs += info.output;
            facts.push(info);
        }
        Ok(Some(Self { names, facts }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair(first: MemberInfo, second: MemberInfo) -> bool {
        let mut facts = [first, second].into_iter();
        GroupPlan::prepare(&["output".into(), "input".into()], |_| facts.next())
            .unwrap()
            .is_some()
    }
    fn member(device: u32) -> MemberInfo {
        MemberInfo {
            device,
            transport: u32::from_be_bytes(*b"bltn"),
            rate: 48_000.0,
            input: 64,
            output: 64,
            alive: 1,
        }
    }
    #[test]
    fn snapshot_abi_matches_native_and_valid_plan_preserves_order() {
        assert_eq!(std::mem::size_of::<MemberInfo>(), 32);
        assert_eq!(std::mem::offset_of!(MemberInfo, rate), 8);
        assert_eq!(std::mem::offset_of!(MemberInfo, alive), 24);
        let names = ["output".into(), "input".into()];
        let mut ids = [7, 9].into_iter();
        let plan = GroupPlan::prepare(&names, |_| Some(member(ids.next().unwrap())))
            .unwrap()
            .unwrap();
        assert_eq!(plan.names[0].to_str().unwrap(), "output");
        assert_eq!(
            plan.facts
                .iter()
                .map(|info| info.device)
                .collect::<Vec<_>>(),
            [7, 9]
        );
    }
    #[test]
    fn invalid_names_and_member_limits_never_query_native() {
        for names in [
            vec![],
            vec!["a".into()],
            vec!["a".into(), "a".into()],
            vec![String::new(), "a".into()],
            (0..33).map(|n| n.to_string()).collect(),
        ] {
            assert!(
                GroupPlan::prepare(&names, |_| panic!("invalid plan queried native"))
                    .unwrap()
                    .is_none()
            );
        }
        assert!(GroupPlan::prepare(&["a\0b".into(), "c".into()], |_| panic!()).is_err());
    }
    #[test]
    fn incompatible_rates_transports_and_device_aliases_never_group() {
        let first = member(1);
        for rate in [
            0.0,
            7999.0,
            44_100.0,
            48_000.5,
            384_001.0,
            f64::NAN,
            f64::INFINITY,
        ] {
            assert!(!pair(first, MemberInfo { rate, ..member(2) }));
        }
        for transport in [0, AGGREGATE_TRANSPORT] {
            assert!(!pair(
                first,
                MemberInfo {
                    transport,
                    ..member(2)
                }
            ));
        }
        assert!(!pair(first, member(1)));
        assert!(!pair(first, member(0)));
        assert!(!pair(
            first,
            MemberInfo {
                alive: 0,
                ..member(2)
            }
        ));
        assert!(
            GroupPlan::prepare(&["a".into(), "b".into()], |_| None)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn channel_totals_are_bounded_without_overflow_and_keep_asymmetry() {
        let first = MemberInfo {
            input: 0,
            output: 2048,
            ..member(1)
        };
        assert!(pair(
            first,
            MemberInfo {
                input: 2048,
                output: 0,
                ..member(2)
            }
        ));
        assert!(!pair(first, member(2)));
        assert!(!pair(
            member(1),
            MemberInfo {
                input: u32::MAX,
                ..member(2)
            }
        ));
        let names = (0..32).map(|n| n.to_string()).collect::<Vec<_>>();
        let mut id = 0;
        assert!(
            GroupPlan::prepare(&names, |_| {
                id += 1;
                Some(member(id))
            })
            .unwrap()
            .is_some()
        );
    }
}
