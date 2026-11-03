use std::ffi::CStr;

use super::{AggregateGeometry, MAX_MEMBERS, MemberInfo, Readback};

mod audio;
mod cf;
mod ffi;
#[cfg(test)]
mod tests;
#[cfg(any(test, feature = "test-observation"))]
pub use ffi::simulated as test_observation;
pub use ffi::{
    mr_aggregate_group_binding, mr_aggregate_group_create, mr_aggregate_group_ready,
    mr_aggregate_group_try_destroy, mr_aggregate_member_info,
};

/// Private aggregate owner. The control-thread retirement API consumes this only
/// after native destruction acknowledges that callbacks can no longer reference it.
#[derive(Debug)]
pub struct AggregateGroup {
    device: u32,
    uids: [Option<cf::Object>; MAX_MEMBERS],
    geometry: AggregateGeometry,
}

/// Checked native device and channel projection returned as one atomic result.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeBinding {
    /// Aggregate device identity.
    pub device: u32,
    /// Selected member's first channel.
    pub offset: u32,
    /// Total channels in the selected direction.
    pub total: u32,
}

impl AggregateGroup {
    fn new(geometry: AggregateGeometry) -> Self {
        Self {
            device: 0,
            uids: std::array::from_fn(|_| None),
            geometry,
        }
    }

    fn uids(&self) -> &[Option<cf::Object>] {
        &self.uids[..self.geometry.count as usize]
    }

    /// Whether each member's freshly `observe`d facts still match the planned geometry.
    fn members_match(&self, observe: impl Fn(&cf::Object) -> Option<MemberInfo>) -> bool {
        self.uids().iter().enumerate().all(|(index, uid)| {
            let Ok(index) = u32::try_from(index) else {
                return false;
            };
            uid.as_ref()
                .and_then(&observe)
                .is_some_and(|observed| self.geometry.member_matches(index, &observed))
        })
    }

    fn description(&self) -> Option<cf::Object> {
        let mut members: [Option<cf::Object>; MAX_MEMBERS] = std::array::from_fn(|_| None);
        for (index, uid) in self.uids().iter().enumerate() {
            let drift = cf::Object::number(i32::from(index != self.geometry.primary as usize))?;
            let quality = cf::Object::number(0x60)?;
            members[index] = cf::Object::dictionary([
                (c"uid", uid.as_ref()?),
                (c"drift", &drift),
                (c"drift quality", &quality),
            ]);
            members[index].as_ref()?;
        }
        let members = cf::Object::array(&members[..self.geometry.count as usize])?;
        let uid = cf::Object::unique_uid()?;
        let name = cf::Object::string(c"MixRelay private I/O")?;
        let private = cf::Object::number(1)?;
        let stacked = cf::Object::number(0)?;
        cf::Object::dictionary([
            (c"uid", &uid),
            (c"name", &name),
            (c"subdevices", &members),
            (
                c"master",
                self.uids[self.geometry.primary as usize].as_ref()?,
            ),
            (c"private", &private),
            (c"stacked", &stacked),
        ])
    }

    fn create(&mut self, create: impl FnOnce(&cf::Object, &mut u32) -> i32) -> bool {
        let Some(description) = self.description() else {
            return false;
        };
        // The owner stores the returned ID before checking the native status:
        // a partially created object must still follow acknowledged retirement.
        create(&description, &mut self.device) == 0
    }

    fn readback(&self, hal: &impl audio::Hal) -> Option<Readback> {
        let uids = hal.object(self.device, *b"grup")?;
        let main_uid = hal.object(self.device, *b"amst")?;
        let composition = hal.object(self.device, *b"acom")?;
        let members = composition.value(c"subdevices")?;
        let count = self.geometry.count as usize;
        if uids.len()? != count || members.len()? != count {
            return None;
        }
        let mut readback = Readback {
            primary_uid_matches: u32::from(
                main_uid.equal(self.uids[self.geometry.primary as usize].as_ref()?),
            ),
            input: hal.channels(self.device, true)?,
            output: hal.channels(self.device, false)?,
            ..Readback::default()
        };
        if !hal.active(self.device, &mut readback.active[..count]) {
            return None;
        }
        for (index, uid) in self.uids().iter().enumerate() {
            let uid = uid.as_ref()?;
            let member = members.element(index)?;
            readback.ordered_uids_match[index] = u8::from(uids.element(index)?.equal(uid));
            readback.composition_uids_match[index] = u8::from(member.value(c"uid")?.equal(uid));
            readback.drift[index] = member.value(c"drift")?.integer()?;
        }
        Some(readback)
    }

    /// Whether `CoreAudio` still publishes the planned members, layout and clock policy.
    fn ready(&self, hal: &impl audio::Hal) -> bool {
        self.device != 0
            && self.members_match(|uid| audio::member(hal, uid, false))
            && self
                .readback(hal)
                .is_some_and(|readback| self.geometry.ready(&readback))
    }

    fn binding(
        &self,
        uid: &CStr,
        input: bool,
        channels: u32,
        hal: &impl audio::Hal,
    ) -> Option<NativeBinding> {
        if !self.ready(hal) {
            return None;
        }
        let uid = cf::Object::string(uid)?;
        let index = self.uids().iter().position(|candidate| {
            candidate
                .as_ref()
                .is_some_and(|candidate| candidate.equal(&uid))
        })?;
        let binding = self
            .geometry
            .binding(u32::try_from(index).ok()?, input, channels)?;
        Some(NativeBinding {
            device: self.device,
            offset: binding.offset,
            total: binding.total,
        })
    }

    fn retire(&mut self, destroy: impl FnOnce(u32) -> i32) -> bool {
        if self.device == 0 {
            return true;
        }
        let result = destroy(self.device);
        // SDK AudioHardwareBase.h: removed objects/devices already relinquish ownership.
        if result != 0
            && result != i32::from_be_bytes(*b"!obj")
            && result != i32::from_be_bytes(*b"!dev")
        {
            return false;
        }
        self.device = 0;
        true
    }
}
