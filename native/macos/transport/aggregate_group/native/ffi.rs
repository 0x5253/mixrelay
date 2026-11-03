use std::{
    alloc::{Layout, alloc},
    ffi::{CStr, c_char, c_void},
};

use super::{
    AggregateGeometry, AggregateGroup, MAX_MEMBERS, MemberInfo, NativeBinding,
    audio::{self, Hal, System},
    cf,
};

#[cfg(any(test, feature = "test-observation"))]
pub mod simulated;
#[cfg(test)]
mod tests;

#[repr(i32)]
#[derive(Debug, PartialEq, Eq)]
enum CreationStatus {
    Created = 0,
    InvalidArgument = 1,
    NativeFailure = 2,
}

/// Queries current physical member facts without modifying the device.
///
/// # Safety
/// `uid` is a terminated string borrowed for this synchronous control-thread query.
/// `info` is exclusive aligned storage for one snapshot, disjoint from `uid`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_aggregate_member_info(
    uid: *const c_char,
    info: *mut MemberInfo,
) -> bool {
    // SAFETY: forwards the caller's UID and result-slot contract.
    unsafe { member_info(&System, uid, info) }
}

/// [`mr_aggregate_member_info`] through `hal`.
/// # Safety
/// Same contract as [`mr_aggregate_member_info`].
unsafe fn member_info(hal: &impl Hal, uid: *const c_char, info: *mut MemberInfo) -> bool {
    if uid.is_null() || info.is_null() {
        return false;
    }
    // SAFETY: the exclusive result slot is cleared even if CF rejects the UID.
    unsafe { info.write(MemberInfo::default()) };
    // SAFETY: the engine owns the C string for the duration of this query.
    let Some(uid) = cf::Object::string(unsafe { CStr::from_ptr(uid) }) else {
        return false;
    };
    let observed = audio::member(hal, &uid, true);
    // SAFETY: the result slot is exclusive, and no reference to it escaped to CoreAudio.
    unsafe { info.write(observed.unwrap_or_default()) };
    observed.is_some()
}

/// Allocates one retirement owner; a non-null return transfers ownership even
/// when native creation fails. Status 1 rejects arguments, 2 reports creation
/// failure, and 0 acknowledges creation (readiness must still be checked).
///
/// # Safety
/// `uids` and `members` hold `count` initialized entries; every UID is terminated.
/// These arrays remain immutable until return, and `status` is exclusive writable
/// storage. The engine supplies its validated unique-member aggregate plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_aggregate_group_create(
    uids: *const *const c_char,
    members: *const MemberInfo,
    count: u32,
    primary: u32,
    status: *mut i32,
) -> *mut AggregateGroup {
    if status.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller's exclusive result slot remains live through construction.
    unsafe { status.write(1) };
    if uids.is_null() || members.is_null() || !(2..=MAX_MEMBERS).contains(&(count as usize)) {
        return std::ptr::null_mut();
    }
    // SAFETY: count was bounded and the caller provides complete immutable arrays.
    let (uids, members) = unsafe {
        (
            std::slice::from_raw_parts(uids, count as usize),
            std::slice::from_raw_parts(members, count as usize),
        )
    };
    // SAFETY: the caller keeps every UID terminated and alive until return.
    let (pointer, result) = unsafe { create(&System, uids, members, primary) };
    // SAFETY: status is disjoint from the owned group and borrowed input arrays.
    unsafe { status.write(result as i32) };
    pointer
}

/// Plans and publishes one private aggregate through `hal`. A non-null owner transfers
/// with every status; null comes only with a rejected geometry or failed allocation.
/// # Safety
/// Every non-null UID is terminated and alive until return.
unsafe fn create(
    hal: &impl Hal,
    uids: &[*const c_char],
    members: &[MemberInfo],
    primary: u32,
) -> (*mut AggregateGroup, CreationStatus) {
    let Some(geometry) = AggregateGeometry::new(members, primary) else {
        return (std::ptr::null_mut(), CreationStatus::InvalidArgument);
    };
    // SAFETY: control-thread owner allocation uses the exact type layout; null is
    // handled before initialization and before creating any native resources.
    // The untyped allocation is aligned for AggregateGroup by that same Layout.
    let pointer = unsafe {
        alloc(Layout::new::<AggregateGroup>())
            .cast::<c_void>()
            .cast::<AggregateGroup>()
    };
    if pointer.is_null() {
        return (pointer, CreationStatus::NativeFailure);
    }
    // SAFETY: newly allocated aligned storage has no aliases; initialization gives
    // the returned pointer exactly one owner, including every failure below.
    unsafe { pointer.write(AggregateGroup::new(geometry)) };
    // SAFETY: the initialized owner is unpublished, so this borrow is exclusive.
    let group = unsafe { &mut *pointer };
    // Each UID is borrowed only when adopted, so earlier members stay owned when a later
    // pointer is rejected.
    let borrowed = uids.iter().map(|&uid| {
        // SAFETY: the caller keeps every non-null UID terminated and alive until return.
        (!uid.is_null()).then(|| unsafe { CStr::from_ptr(uid) })
    });
    let result = initialize(
        group,
        borrowed,
        |uid| audio::member(hal, uid, false),
        |description, device| hal.create(description, device),
    );
    (pointer, result)
}

/// Adopts every UID (`None` is a null argument), then revalidates the engine's member facts
/// with `observe` before `create` may publish any native device. A stale plan never reaches
/// native creation.
fn initialize<'a>(
    group: &mut AggregateGroup,
    uids: impl IntoIterator<Item = Option<&'a CStr>>,
    observe: impl Fn(&cf::Object) -> Option<MemberInfo>,
    create: impl FnOnce(&cf::Object, &mut u32) -> i32,
) -> CreationStatus {
    for (slot, uid) in group.uids.iter_mut().zip(uids) {
        let Some(uid) = uid.filter(|uid| !uid.is_empty()) else {
            return CreationStatus::InvalidArgument;
        };
        *slot = cf::Object::string(uid);
        if slot.is_none() {
            return CreationStatus::NativeFailure;
        }
    }
    if !group.members_match(observe) {
        return CreationStatus::NativeFailure;
    }
    if group.create(create) {
        CreationStatus::Created
    } else {
        CreationStatus::NativeFailure
    }
}

/// Attempts one native destruction. True consumes ownership; false preserves
/// every native and CF resource for a later control-thread retirement attempt.
///
/// # Safety
/// `group` is null or the unique owner returned by create; device callbacks have
/// drained and no concurrent readiness/binding query uses the pointer. A pointer
/// consumed by a successful call must never be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_aggregate_group_try_destroy(group: *mut AggregateGroup) -> bool {
    // SAFETY: forwards the caller's exclusive retirement contract.
    unsafe { try_destroy(&System, group) }
}

/// [`mr_aggregate_group_try_destroy`] through `hal`.
/// # Safety
/// Same contract as [`mr_aggregate_group_try_destroy`].
unsafe fn try_destroy(hal: &impl Hal, group: *mut AggregateGroup) -> bool {
    // SAFETY: control-thread retirement holds exclusive access after callback drain.
    let Some(owner) = (unsafe { group.as_mut() }) else {
        return true;
    };
    if !owner.retire(|device| hal.destroy(device)) {
        return false;
    }
    // SAFETY: native destruction succeeded, ending all native references; this
    // pointer came from alloc with Box's exact layout and has one remaining owner.
    unsafe { drop(Box::from_raw(group)) };
    true
}

/// Confirms current member identities, geometry and OS-accepted clock policy.
///
/// # Safety
/// `group` is null or an initialized owner retained until this synchronous query
/// returns. No retirement or creation mutation runs concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_aggregate_group_ready(group: *const AggregateGroup) -> bool {
    // SAFETY: forwards the caller's retained-owner contract.
    unsafe { ready(&System, group) }
}

/// [`mr_aggregate_group_ready`] through `hal`.
/// # Safety
/// Same contract as [`mr_aggregate_group_ready`].
unsafe fn ready(hal: &impl Hal, group: *const AggregateGroup) -> bool {
    // SAFETY: the engine keeps its control-thread retirement owner live through query.
    unsafe { group.as_ref() }.is_some_and(|group| group.ready(hal))
}

/// Returns a member projection only after the entire aggregate is ready.
/// Failure leaves the caller's result unchanged.
///
/// # Safety
/// `group` remains owned and immutable until return; `uid` is a terminated string.
/// `output` is exclusive aligned storage for one binding, disjoint from the group
/// and UID. No retirement runs concurrently with this control-thread query.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_aggregate_group_binding(
    group: *const AggregateGroup,
    uid: *const c_char,
    input: bool,
    channels: u32,
    output: *mut NativeBinding,
) -> bool {
    if output.is_null() {
        return false;
    }
    // SAFETY: forwards the caller's owner and UID contract.
    let Some(binding) = (unsafe { binding(&System, group, uid, input, channels) }) else {
        return false;
    };
    // SAFETY: output is separate writable storage and is committed only on success.
    unsafe { output.write(binding) };
    true
}

/// A member's projection through `hal`, only after the entire aggregate is ready.
/// # Safety
/// `group` is null or remains owned and immutable until return; `uid` is null or terminated.
unsafe fn binding(
    hal: &impl Hal,
    group: *const AggregateGroup,
    uid: *const c_char,
    input: bool,
    channels: u32,
) -> Option<NativeBinding> {
    if uid.is_null() {
        return None;
    }
    // SAFETY: control-thread caller retains initialized group and UID storage; the UID is
    // borrowed for this query, not retained by the native owner.
    let (group, uid) = unsafe { (group.as_ref()?, CStr::from_ptr(uid)) };
    group.binding(uid, input, channels, hal)
}
