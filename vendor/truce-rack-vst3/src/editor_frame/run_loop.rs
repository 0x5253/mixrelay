//! Own Linux editor event/timer registration, readiness, and callback dispatch.

use std::{
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use vst3::{
    ComPtr, ComRef,
    Steinberg::{
        Linux::{IEventHandler, IEventHandlerTrait, ITimerHandler, ITimerHandlerTrait},
        kInvalidArgument, kResultFalse, kResultOk,
    },
};
const MAX_EVENT_HANDLERS: usize = 64;
const MAX_TIMER_HANDLERS: usize = 32;

struct EventRegistration {
    id: u64,
    fd: libc::c_int,
    handler: ComPtr<IEventHandler>,
}

struct TimerRegistration {
    id: u64,
    interval: Duration,
    due: Instant,
    handler: ComPtr<ITimerHandler>,
}

struct Registrations {
    events: Vec<EventRegistration>,
    timers: Vec<TimerRegistration>,
    next_id: u64,
    closed: bool,
}

impl Registrations {
    fn new() -> Self {
        Self {
            events: Vec::with_capacity(MAX_EVENT_HANDLERS),
            timers: Vec::with_capacity(MAX_TIMER_HANDLERS),
            next_id: 1,
            closed: false,
        }
    }

    fn next_id(&mut self) -> Option<u64> {
        let id = self.next_id;
        self.next_id = id.checked_add(1)?;
        Some(id)
    }
}

pub(super) struct RunLoopState {
    registrations: Mutex<Registrations>,
    servicing: AtomicBool,
}

impl RunLoopState {
    pub(super) fn new() -> Self {
        Self {
            registrations: Mutex::new(Registrations::new()),
            servicing: AtomicBool::new(false),
        }
    }

    fn registrations(&self) -> MutexGuard<'_, Registrations> {
        self.registrations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn register_event_handler(
        &self,
        handler: *mut IEventHandler,
        fd: libc::c_int,
    ) -> i32 {
        if handler.is_null() || fd < 0 {
            return kInvalidArgument;
        }
        // SAFETY: IRunLoop receives a live borrowed interface pointer for this call; upgrading
        // it retains the callback until unregister or editor teardown.
        let Some(handler_ref) = (unsafe { ComRef::from_raw(handler) }) else {
            return kInvalidArgument;
        };
        let owned_handler = handler_ref.to_com_ptr();
        let mut registrations = self.registrations();
        if registrations.closed {
            return kResultFalse;
        }
        if let Some(existing) = registrations
            .events
            .iter()
            .find(|registration| registration.handler.as_ptr() == handler)
        {
            return if existing.fd == fd {
                kResultOk
            } else {
                kInvalidArgument
            };
        }
        if registrations.events.len() == MAX_EVENT_HANDLERS {
            return kResultFalse;
        }
        let Some(id) = registrations.next_id() else {
            return kResultFalse;
        };
        registrations.events.push(EventRegistration {
            id,
            fd,
            handler: owned_handler,
        });
        kResultOk
    }

    pub(super) fn unregister_event_handler(&self, handler: *mut IEventHandler) -> i32 {
        if handler.is_null() {
            return kInvalidArgument;
        }
        let removed = {
            let mut registrations = self.registrations();
            registrations
                .events
                .iter()
                .position(|registration| registration.handler.as_ptr() == handler)
                .map(|index| registrations.events.swap_remove(index))
        };
        drop(removed);
        kResultOk
    }

    pub(super) fn register_timer(&self, handler: *mut ITimerHandler, milliseconds: u64) -> i32 {
        if handler.is_null() {
            return kInvalidArgument;
        }
        let interval = Duration::from_millis(milliseconds.max(1));
        let now = Instant::now();
        let Some(due) = now.checked_add(interval) else {
            return kInvalidArgument;
        };
        // SAFETY: IRunLoop receives a live borrowed interface pointer for this call; upgrading
        // it retains the callback until unregister or editor teardown.
        let Some(handler_ref) = (unsafe { ComRef::from_raw(handler) }) else {
            return kInvalidArgument;
        };
        let owned_handler = handler_ref.to_com_ptr();
        let mut registrations = self.registrations();
        if registrations.closed {
            return kResultFalse;
        }
        if registrations
            .timers
            .iter()
            .any(|registration| registration.handler.as_ptr() == handler)
        {
            return kResultOk;
        }
        if registrations.timers.len() == MAX_TIMER_HANDLERS {
            return kResultFalse;
        }
        let Some(id) = registrations.next_id() else {
            return kResultFalse;
        };
        registrations.timers.push(TimerRegistration {
            id,
            interval,
            due,
            handler: owned_handler,
        });
        kResultOk
    }

    pub(super) fn unregister_timer(&self, handler: *mut ITimerHandler) -> i32 {
        if handler.is_null() {
            return kInvalidArgument;
        }
        let removed = {
            let mut registrations = self.registrations();
            registrations
                .timers
                .iter()
                .position(|registration| registration.handler.as_ptr() == handler)
                .map(|index| registrations.timers.swap_remove(index))
        };
        drop(removed);
        kResultOk
    }

    pub(super) fn close(&self) {
        let (events, timers) = {
            let mut registrations = self.registrations();
            registrations.closed = true;
            (
                std::mem::take(&mut registrations.events),
                std::mem::take(&mut registrations.timers),
            )
        };
        drop((events, timers));
    }

    pub(super) fn service(&self) {
        if self
            .servicing
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let _guard = ServiceGuard(&self.servicing);
        let Some((events, due_timers)) = self.snapshot() else {
            return;
        };
        self.dispatch_events(&events);
        // Release event COM snapshots before invoking timer callbacks.
        drop(events);
        self.dispatch_timers(due_timers);
    }

    fn snapshot(&self) -> Option<(Vec<EventCall>, Vec<TimerCall>)> {
        let registrations = self.registrations();
        if registrations.closed {
            return None;
        }
        let now = Instant::now();
        let events = registrations
            .events
            .iter()
            .map(|registration| EventCall {
                id: registration.id,
                fd: registration.fd,
                handler: registration.handler.clone(),
            })
            .collect();
        let due_timers = registrations
            .timers
            .iter()
            .filter(|registration| registration.due <= now)
            .map(|registration| TimerCall {
                id: registration.id,
                due: registration.due,
                interval: registration.interval,
                handler: registration.handler.clone(),
            })
            .collect();
        Some((events, due_timers))
    }

    fn dispatch_events(&self, events: &[EventCall]) {
        let mut descriptors = events
            .iter()
            .map(|event| libc::pollfd {
                fd: event.fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect::<Vec<_>>();
        if !descriptors.is_empty() {
            // SAFETY: the initialized descriptor array is writable and the timeout is zero, so
            // this readiness check cannot block the plugin control/UI thread.
            let ready = unsafe {
                libc::poll(
                    descriptors.as_mut_ptr(),
                    match libc::nfds_t::try_from(descriptors.len()) {
                        Ok(count) => count,
                        Err(_) => return,
                    },
                    0,
                )
            };
            if ready > 0 {
                for (event, descriptor) in events.iter().zip(&descriptors) {
                    if descriptor.revents & libc::POLLIN != 0
                        && self.event_is_registered(event.id, event.fd)
                    {
                        // SAFETY: each snapshot owns a COM reference. No registry lock is held
                        // across plugin code, so callbacks may safely unregister themselves.
                        unsafe { event.handler.onFDIsSet(event.fd) };
                    }
                }
            }
        }
    }

    fn dispatch_timers(&self, timers: Vec<TimerCall>) {
        for timer in timers {
            let now = Instant::now();
            if self.advance_timer(timer.id, timer.due, timer.interval, now) {
                // SAFETY: the snapshot owns a COM reference and no registry lock is held.
                unsafe { timer.handler.onTimer() };
            }
        }
    }

    fn event_is_registered(&self, id: u64, fd: libc::c_int) -> bool {
        self.registrations()
            .events
            .iter()
            .any(|registration| registration.id == id && registration.fd == fd)
    }

    fn advance_timer(
        &self,
        id: u64,
        expected_due: Instant,
        interval: Duration,
        now: Instant,
    ) -> bool {
        let removed =
            {
                let mut registrations = self.registrations();
                let Some(index) = registrations.timers.iter().position(|registration| {
                    registration.id == id && registration.due == expected_due
                }) else {
                    return false;
                };
                if let Some(next_due) = now.checked_add(interval) {
                    registrations.timers[index].due = next_due;
                    None
                } else {
                    Some(registrations.timers.swap_remove(index))
                }
            };
        let advanced = removed.is_none();
        drop(removed);
        advanced
    }
}

struct EventCall {
    id: u64,
    fd: libc::c_int,
    handler: ComPtr<IEventHandler>,
}

struct TimerCall {
    id: u64,
    due: Instant,
    interval: Duration,
    handler: ComPtr<ITimerHandler>,
}

struct ServiceGuard<'a>(&'a AtomicBool);

impl Drop for ServiceGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
