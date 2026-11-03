//! The private-aggregate entry points beneath a [`SimulatedHal`]. Creation, revalidation,
//! readback, projection and retirement run the production code; only the `CoreAudio`
//! calls below it reach the simulated objects, which republish exactly what was created.

use std::ffi::{CStr, CString, c_char};

use super::super::{
    super::MAX_MEMBERS, AggregateGroup, MemberInfo, NativeBinding, audio::Hal, cf::Object,
};
use super::CreationStatus;
use crate::device_io::test_observation::{Composition, SimulatedHal};

impl SimulatedHal {
    /// The present device whose UID equals `uid`.
    fn named(&self, uid: &Object) -> Option<(String, u32)> {
        self.devices().into_iter().find(|(name, _)| {
            CString::new(name.as_str())
                .ok()
                .and_then(|name| Object::string(&name))
                .is_some_and(|name| name.equal(uid))
        })
    }

    /// The composition a description requests: its members in order, drift flags and master.
    fn requested(&self, description: &Object) -> Option<Composition> {
        let private = description.value(c"private")?.integer()? == 1;
        let stacked = description.value(c"stacked")?.integer()? != 0;
        if !private || stacked {
            return None;
        }
        let subdevices = description.value(c"subdevices")?;
        let members = (0..subdevices.len()?)
            .map(|index| {
                let member = subdevices.element(index)?;
                let (uid, _) = self.named(&member.value(c"uid")?)?;
                Some((uid, member.value(c"drift")?.integer()?))
            })
            .collect::<Option<Vec<_>>>()?;
        let (master, _) = self.named(&description.value(c"master")?)?;
        Some(Composition { members, master })
    }
}

impl Hal for SimulatedHal {
    fn device(&self, uid: &Object) -> Option<u32> {
        self.named(uid).map(|(_, device)| device)
    }

    fn scalar(&self, device: u32, selector: [u8; 4]) -> Option<u32> {
        SimulatedHal::scalar(self, device, selector)
    }

    fn rate(&self, device: u32) -> Option<f64> {
        SimulatedHal::rate(self, device)
    }

    fn channels(&self, device: u32, _input: bool) -> Option<u32> {
        SimulatedHal::channels(self, device)
    }

    fn object(&self, device: u32, selector: [u8; 4]) -> Option<Object> {
        let composition = self.composition(device)?;
        let string = |uid: &str| Object::string(&CString::new(uid).ok()?);
        match &selector {
            b"grup" => Object::array(
                &composition
                    .members
                    .iter()
                    .map(|(uid, _)| string(uid))
                    .collect::<Vec<_>>(),
            ),
            b"amst" => string(&composition.master),
            b"acom" => {
                let members = composition
                    .members
                    .iter()
                    .map(|(uid, drift)| {
                        Object::dictionary([
                            (c"uid", &string(uid)?),
                            (c"drift", &Object::number(*drift)?),
                        ])
                    })
                    .collect::<Vec<_>>();
                Object::dictionary([(c"subdevices", &Object::array(&members)?)])
            }
            _ => None,
        }
    }

    fn active(&self, device: u32, output: &mut [u32]) -> bool {
        let Some(active) = SimulatedHal::active(self, device) else {
            return false;
        };
        if active.len() != output.len() {
            return false;
        }
        output.copy_from_slice(&active);
        true
    }

    fn create(&self, description: &Object, device: &mut u32) -> i32 {
        let Some(composition) = self.requested(description) else {
            return i32::from_be_bytes(*b"nope");
        };
        match self.create_aggregate(composition) {
            Ok(created) => {
                *device = created;
                0
            }
            Err(status) => status,
        }
    }

    fn destroy(&self, device: u32) -> i32 {
        self.destroy_aggregate(device)
    }
}

/// `mr_aggregate_member_info` beneath `hal`.
/// # Safety
/// `uid` is null or terminated for this query; `info` is null or exclusive aligned storage.
pub unsafe fn member_info(hal: &SimulatedHal, uid: *const c_char, info: *mut MemberInfo) -> bool {
    // SAFETY: forwards the caller's UID and result-slot contract.
    unsafe { super::member_info(hal, uid, info) }
}

/// `mr_aggregate_group_create` beneath `hal`, for `uids.len()` members. Returns the owner,
/// which transfers with every status, and that status.
/// # Safety
/// `members` holds `uids.len()` initialized entries; every non-null UID is terminated. Both
/// stay immutable until return.
pub unsafe fn create(
    hal: &SimulatedHal,
    uids: &[*const c_char],
    members: *const MemberInfo,
    primary: u32,
) -> (*mut AggregateGroup, i32) {
    if members.is_null() || !(2..=MAX_MEMBERS).contains(&uids.len()) {
        return (std::ptr::null_mut(), CreationStatus::InvalidArgument as i32);
    }
    // SAFETY: the caller provides one initialized snapshot per UID.
    let members = unsafe { std::slice::from_raw_parts(members, uids.len()) };
    // SAFETY: the caller keeps every UID terminated and alive until return.
    let (group, status) = unsafe { super::create(hal, uids, members, primary) };
    (group, status as i32)
}

/// `mr_aggregate_group_ready` beneath `hal`.
/// # Safety
/// `group` is null or an initialized owner retained until return.
pub unsafe fn ready(hal: &SimulatedHal, group: *const AggregateGroup) -> bool {
    // SAFETY: forwards the caller's retained-owner contract.
    unsafe { super::ready(hal, group) }
}

/// `mr_aggregate_group_binding` beneath `hal`.
/// # Safety
/// `group` is null or remains owned and immutable until return.
pub unsafe fn binding(
    hal: &SimulatedHal,
    group: *const AggregateGroup,
    uid: &CStr,
    input: bool,
    channels: u32,
) -> Option<NativeBinding> {
    // SAFETY: forwards the caller's owner contract with a terminated, borrowed UID.
    unsafe { super::binding(hal, group, uid.as_ptr(), input, channels) }
}

/// `mr_aggregate_group_try_destroy` beneath `hal`.
/// # Safety
/// `group` is null or the unique owner from [`create`] after callbacks drained; a consumed
/// pointer is never used again.
pub unsafe fn try_destroy(hal: &SimulatedHal, group: *mut AggregateGroup) -> bool {
    // SAFETY: forwards the caller's exclusive retirement contract.
    unsafe { super::try_destroy(hal, group) }
}
