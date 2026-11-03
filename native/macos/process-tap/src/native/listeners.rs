//! Native listener receipts and immediate atomic invalidation of tap delivery.

use super::{Address, AudioObjectAddPropertyListener, TapError, TapSession, context};
use crate::control_trace::{self, Procedure};
use std::ffi::c_void;

pub(super) unsafe extern "C" fn format_changed(
    _: u32,
    _: u32,
    _: *const Address,
    context: *mut c_void,
) -> i32 {
    // SAFETY: registration retains session until removal succeeds and active listeners drain.
    let session = unsafe { &*context.cast::<TapSession>() };
    let _active = crate::ffi::Active::enter(&session.active_listeners);
    // SAFETY: listener touches only the initialized atomic invalidation gate.
    session.lifecycle.changed();
    0
}
/// Atomically invalidates delivery before the off-RT control owner rescans.
pub(super) unsafe extern "C" fn inventory_changed(
    _: u32,
    _: u32,
    _: *const Address,
    context: *mut c_void,
) -> i32 {
    // SAFETY: registration retains the pinned session through removal and drain.
    let session = unsafe { &*context.cast::<TapSession>() };
    let _active = crate::ffi::Active::enter(&session.active_listeners);
    session
        .inventory_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    session.lifecycle.changed();
    0
}

pub(crate) fn add_inventory_listener(session: &TapSession) -> Result<(), TapError> {
    control_trace::run(Procedure::AddInventoryListener, || {
        // SAFETY: pinned session is initialized before the listener is published.
        TapError::status(unsafe {
            AudioObjectAddPropertyListener(
                1,
                &Address::global(*b"prs#"),
                inventory_changed,
                context(session),
            )
        })?;
        let mut resources = session.resources.get();
        resources.listeners.inventory = true;
        session.resources.set(resources);
        Ok(())
    })
}

pub(super) unsafe extern "C" fn service_changed(
    _: u32,
    _: u32,
    _: *const Address,
    context: *mut c_void,
) -> i32 {
    // SAFETY: registered session remains pinned through removal and listener drain.
    let session = unsafe { &*context.cast::<TapSession>() };
    let _active = crate::ffi::Active::enter(&session.active_listeners);
    service_restarted(session);
    0
}

pub(crate) fn service_restarted(session: &TapSession) {
    let _ = session.service_epoch.fetch_update(
        std::sync::atomic::Ordering::Release,
        std::sync::atomic::Ordering::Relaxed,
        |epoch| Some(epoch.saturating_add(1)),
    );
    session.lifecycle.changed();
}

pub(crate) fn add_service_listener(session: &TapSession) -> Result<(), TapError> {
    control_trace::run(Procedure::AddServiceListener, || {
        // SAFETY: caller owns a fully initialized, pinned session before publication.
        TapError::status(unsafe {
            AudioObjectAddPropertyListener(
                1,
                &Address::global(*b"srst"),
                service_changed,
                context(session),
            )
        })?;
        let mut resources = session.resources.get();
        resources.listeners.service = true;
        session.resources.set(resources);
        Ok(())
    })
}

pub(crate) fn add_listener(session: &TapSession) -> Result<(), TapError> {
    control_trace::run(Procedure::AddFormatListener, || {
        let mut resources = session.resources.get();
        // SAFETY: pinned session initialized before publication; failed removal retains it.
        TapError::status(unsafe {
            AudioObjectAddPropertyListener(
                resources.tap,
                &Address::global(*b"tfmt"),
                format_changed,
                context(session),
            )
        })?;
        resources.listeners.format = true;
        session.resources.set(resources);
        Ok(())
    })
}
