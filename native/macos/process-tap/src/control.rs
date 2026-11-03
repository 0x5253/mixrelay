use super::{
    RebuildIntent, TapError, TapSession,
    native::{Native, ProcessIdentity},
};
use crate::control_trace::{self, Procedure};
use crate::process_tap_input::TapFormat;
use mixrelay_macos_transport::capture_ring::mr_audio_ring_read_interleaved;
use std::{ptr, sync::atomic::Ordering};

struct Discovery {
    candidate: Vec<ProcessIdentity>,
    retrack: bool,
}

// A deleted native object cannot retain a registration. Other failures still
// retain the exact owner for retry, and active callbacks/listeners must drain.
fn removed(result: Result<(), TapError>) -> bool {
    match result {
        Ok(()) => true,
        Err(error) => {
            error.code == 4
                && (error.os_status == i32::from_be_bytes(*b"!obj")
                    || error.os_status == i32::from_be_bytes(*b"!dev"))
        }
    }
}

impl TapSession {
    pub(super) fn adopt_registration(
        &self,
        proc: *mut std::ffi::c_void,
        status: i32,
    ) -> Result<(), TapError> {
        let mut resources = self.resources.get();
        resources.proc = proc;
        self.resources.set(resources);
        TapError::status(status)?;
        if proc.is_null() {
            Err(TapError::code(4))
        } else {
            Ok(())
        }
    }
    pub(super) fn start(&self, native: &impl Native) -> Result<(), TapError> {
        if self.running.load(Ordering::Acquire) {
            return Err(TapError::code(7));
        }
        let mut resources = self.resources.get();
        if resources.proc.is_null()
            || !self.allowed()
            || self.active_callbacks.load(Ordering::Acquire) != 0
        {
            return Err(TapError::code(9));
        }
        self.watchdog.borrow_mut().clear(native);
        if resources.start_attempted {
            self.discard_pcm();
            self.generation.fetch_add(1, Ordering::Release);
        }
        resources.start_attempted = true;
        self.resources.set(resources);
        // A successful Start may call the IOProc before returning. A failed
        // Start retains its owner but must not expose any partially accepted PCM.
        self.delivery.store(true, Ordering::Release);
        if let Err(error) = native.start(resources.aggregate, resources.proc) {
            self.delivery.store(false, Ordering::Release);
            return Err(error);
        }
        self.running.store(true, Ordering::Release);
        Ok(())
    }

    pub(super) fn stop(&self, native: &impl Native) -> Result<(), TapError> {
        self.watchdog.borrow_mut().clear(native);
        if !self.running.load(Ordering::Acquire) {
            return Err(TapError::code(8));
        }
        let mut resources = self.resources.get();
        // Closing admission precedes native Stop. An already entered callback
        // remains owned; restart cannot discard its ring until its guard drains.
        self.delivery.store(false, Ordering::Release);
        native.stop(resources.aggregate, resources.proc)?;
        self.running.store(false, Ordering::Release);
        resources.resume = false;
        self.resources.set(resources);
        Ok(())
    }

    fn unregister(&self, native: &impl Native) -> bool {
        self.delivery.store(false, Ordering::Release);
        let mut resources = self.resources.get();
        if self.running.load(Ordering::Acquire)
            && removed(native.stop(resources.aggregate, resources.proc))
        {
            self.running.store(false, Ordering::Release);
        }
        if !resources.proc.is_null() && resources.aggregate != 0 {
            if !removed(native.unregister(resources.aggregate, resources.proc)) {
                return false;
            }
            resources.proc = std::ptr::null_mut();
            self.resources.set(resources);
        }
        self.running.store(false, Ordering::Release);
        self.active_callbacks.load(Ordering::Acquire) == 0
    }

    pub(super) fn reclaim(&self, native: &impl Native) -> bool {
        self.watchdog.borrow_mut().clear(native);
        self.remove_system_listeners(native) && self.reclaim_tap(native)
    }

    fn remove_system_listeners(&self, native: &impl Native) -> bool {
        let mut resources = self.resources.get();
        if resources.listeners.service {
            if !removed(native.remove_service_listener(self)) {
                return false;
            }
            resources.listeners.service = false;
            self.resources.set(resources);
        }
        if resources.listeners.inventory {
            if !removed(native.remove_inventory_listener(self)) {
                return false;
            }
            resources.listeners.inventory = false;
            self.resources.set(resources);
        }
        true
    }

    fn restore_service(&self, native: &impl Native) -> Result<(), TapError> {
        let epoch = self.service_epoch.load(Ordering::Acquire);
        if epoch == self.rebuilt_service_epoch.get() {
            return Ok(());
        }
        self.watchdog.borrow_mut().clear(native);
        let mut resources = self.resources.get();
        resources.rebuild = RebuildIntent::Replace;
        resources.resume |= self.running.load(Ordering::Acquire);
        self.resources.set(resources);
        if resources.listeners.service_epoch == epoch {
            return Ok(());
        }
        // The SDK invalidates cached objects AND listeners across a service reset.
        // Partial removals/additions retain individual receipts for the next pass.
        if !self.remove_system_listeners(native)
            || self.active_listeners.load(Ordering::Acquire) != 0
        {
            return Err(TapError::code(9));
        }
        native.install_system_listeners(self)?;
        let mut resources = self.resources.get();
        resources.listeners.service_epoch = epoch;
        self.resources.set(resources);
        Ok(())
    }

    fn reclaim_tap(&self, native: &impl Native) -> bool {
        if !self.unregister(native) {
            return false;
        }
        let mut resources = self.resources.get();
        if resources.listeners.format {
            if !removed(native.remove_listener(resources.tap, self)) {
                return false;
            }
            resources.listeners.format = false;
            self.resources.set(resources);
        }
        if self.active_listeners.load(Ordering::Acquire) != 0 {
            return false;
        }
        if resources.aggregate != 0 {
            if !removed(native.destroy_aggregate(resources.aggregate)) {
                return false;
            }
            resources.aggregate = 0;
            self.resources.set(resources);
        }
        if resources.tap != 0 {
            if !removed(native.destroy_tap(resources.tap)) {
                return false;
            }
            resources.tap = 0;
            self.resources.set(resources);
        }
        true
    }

    pub(super) fn refresh(&self, native: &impl Native) -> Result<(), TapError> {
        self.refresh_at(native, std::time::Instant::now())
    }

    pub(super) fn refresh_at(
        &self,
        native: &impl Native,
        now: std::time::Instant,
    ) -> Result<(), TapError> {
        self.refresh_native_at(native, now).map_err(|error| {
            if self.resources.get().resume && matches!(error.code, 4 | 6) {
                // A suspended producer retains its ownership and retries the
                // partially completed rebuild. The desktop pump must not exit
                // permanently after a transient native/allocation failure.
                TapError { code: 9, ..error }
            } else {
                error
            }
        })
    }

    fn refresh_native_at(
        &self,
        native: &impl Native,
        now: std::time::Instant,
    ) -> Result<(), TapError> {
        // SAFETY: serialized control retains this owner; lifecycle itself is atomic.
        if self.lifecycle.retiring() {
            return Err(TapError::code(1));
        }
        let observed = self.lifecycle.epoch();
        self.restore_service(native)?;
        let discovery = self
            .discover(native)
            .inspect_err(|_| self.watchdog.borrow_mut().clear(native))?;
        let bundle_recertification = discovery.is_some();
        if let Some(Discovery {
            candidate,
            retrack: true,
        }) = discovery
        {
            return self.retrack(native, observed, candidate);
        }
        // SAFETY: gate remains live throughout native format discovery and commit.
        let result = native.format(self.resources.get().tap);
        let compatible = result.as_ref().is_ok_and(|format| self.compatible(format));
        let unchanged = result.as_ref().is_ok_and(|format| {
            format.sample_rate.to_bits() == self.sample_rate.load(Ordering::Acquire)
        });
        // SAFETY: only atomic gate publication overlaps native listeners.
        if compatible
            && unchanged
            && (self.lifecycle.current(observed) || bundle_recertification)
            && (!self.resources.get().resume || self.running.load(Ordering::Acquire))
        {
            // The same exact process identities and unchanged format require
            // only epoch re-certification, never a hardware restart. A newer
            // notification during discovery makes this commit fail safely.
            if self.lifecycle.current(observed) || self.lifecycle.commit(observed) {
                return self.recover_if_confirmed(native, observed, now);
            }
            return Err(TapError::code(9));
        }
        self.watchdog.borrow_mut().clear(native);
        if self.lifecycle.epoch() != observed {
            return Err(TapError::code(9));
        }
        // SAFETY: invalidation precedes unregister and prevents new ring producers.
        let epoch = self.lifecycle.invalidate(observed);
        let mut resources = self.resources.get();
        resources.resume |= self.running.load(Ordering::Acquire);
        self.resources.set(resources);
        if !self.unregister(native) {
            return Err(TapError::code(9));
        }
        let format = result?;
        if !compatible {
            return Err(TapError::code(5));
        }
        self.prepare_format(native, &format)?;
        // SAFETY: old callback drained; new proc is not started until epoch certification.
        if !self.lifecycle.commit(epoch) {
            return Err(TapError::code(9));
        }
        if self.resources.get().resume {
            if let Err(error) = self.start(native) {
                // SAFETY: failed native start must keep delivery invalid until next refresh.
                self.lifecycle.changed();
                return Err(error);
            }
            let mut resources = self.resources.get();
            resources.resume = false;
            self.resources.set(resources);
        }
        Ok(())
    }

    fn recover_if_confirmed(
        &self,
        native: &impl Native,
        observed: u64,
        now: std::time::Instant,
    ) -> Result<(), TapError> {
        if !self.watchdog.borrow_mut().poll(native, self, now) {
            return Ok(());
        }
        let selected = self
            .selection
            .borrow()
            .as_ref()
            .map(|selection| selection.selected.clone())
            .ok_or(TapError::code(1))?;
        self.retrack(native, observed, selected)
    }

    fn discover(&self, native: &impl Native) -> Result<Option<Discovery>, TapError> {
        let selection = self.selection.borrow();
        let Some(selection) = selection.as_ref() else {
            return Ok(None);
        };
        if !selection.fallback {
            // Native bundle selectors restore process membership themselves, but
            // a failed watchdog rebuild still needs its missing native handles
            // recreated on the next control pass.
            return Ok(self.incomplete_tap().then(|| Discovery {
                candidate: selection.selected.clone(),
                retrack: true,
            }));
        }
        if let (Some(uid), Some(device)) = (&selection.target_device_uid, selection.target_device) {
            let current = native
                .device(uid)
                .inspect_err(|_| self.lifecycle.changed())?;
            if current != device {
                self.lifecycle.changed();
                return Err(TapError::code(9));
            }
        }
        let inventory_epoch = self.inventory_epoch.load(Ordering::Acquire);
        let candidate = selection
            .processes(native)
            .inspect_err(|_| self.lifecycle.changed())?;
        if inventory_epoch != self.inventory_epoch.load(Ordering::Acquire) {
            return Err(TapError::code(9));
        }
        if selection.target_device.is_some() && candidate != selection.selected {
            // A scoped mute is a lease on the admitted process objects, not a
            // bundle-wide permission to suppress later routes or replacements.
            self.lifecycle.changed();
            return Err(TapError::code(9));
        }
        let retrack = candidate != selection.selected || self.incomplete_tap();
        Ok(Some(Discovery { candidate, retrack }))
    }

    fn incomplete_tap(&self) -> bool {
        let resources = self.resources.get();
        resources.tap == 0
            || resources.aggregate == 0
            || !resources.listeners.format
            || resources.proc.is_null()
            || resources.rebuild == RebuildIntent::Replace
    }

    fn retrack(
        &self,
        native: &impl Native,
        observed: u64,
        candidate: Vec<ProcessIdentity>,
    ) -> Result<(), TapError> {
        let service_epoch = self.service_epoch.load(Ordering::Acquire);
        self.watchdog.borrow_mut().clear(native);
        // No callback may deliver old-identity PCM after this invalidation.
        let epoch = self.lifecycle.invalidate(observed);
        let mut resources = self.resources.get();
        resources.resume |= self.running.load(Ordering::Acquire);
        resources.rebuild = RebuildIntent::Replace;
        self.resources.set(resources);
        if !self.reclaim_tap(native) {
            return Err(TapError::code(9));
        }
        self.discard_pcm();
        let mut selection = self.selection.borrow_mut();
        let selection = selection.as_mut().ok_or(TapError::code(1))?;
        selection.selected = candidate;
        if selection.fallback && selection.exclusions.is_none() && selection.selected.is_empty() {
            // No tap with an empty process list is ever created: the native
            // initializer is not assumed to mean silence for that case.
            return Err(TapError::code(9));
        }
        native.rebuild(self, selection)?;
        let format = control_trace::run(Procedure::ReadTapFormat, || {
            native.format(self.resources.get().tap)
        })?;
        if !self.compatible(&format) {
            return Err(TapError::code(5));
        }
        self.prepare_format(native, &format)?;
        if !self.lifecycle.commit(epoch) {
            return Err(TapError::code(9));
        }
        if self.resources.get().resume {
            if let Err(error) = self.start(native) {
                self.lifecycle.changed();
                return Err(error);
            }
            let mut resources = self.resources.get();
            resources.resume = false;
            self.resources.set(resources);
        }
        let mut resources = self.resources.get();
        resources.rebuild = RebuildIntent::Unchanged;
        self.resources.set(resources);
        self.rebuilt_service_epoch.set(service_epoch);
        Ok(())
    }

    fn compatible(&self, format: &TapFormat) -> bool {
        self.format
            .get()
            .is_some_and(|current| format.supported() && format.same_geometry(current))
    }

    fn prepare_format(&self, native: &impl Native, format: &TapFormat) -> Result<(), TapError> {
        self.discard_pcm();
        native.register(self)?;
        self.sample_rate
            .store(format.sample_rate.to_bits(), Ordering::Release);
        self.generation.fetch_add(1, Ordering::Release);
        let mut resources = self.resources.get();
        resources.start_attempted = false;
        self.resources.set(resources);
        Ok(())
    }

    fn discard_pcm(&self) {
        let mut discarded = [0.0; 64 * 64];
        // SAFETY: native stop/unregistration and the active count have drained the
        // producer; control owns the sole consumer and scratch covers 64 channels.
        while unsafe { mr_audio_ring_read_interleaved(self.ring(), discarded.as_mut_ptr(), 64) }
            != 0
        {}
        self.peaks.clear();
        // SAFETY: the callback writer has drained before this sole control owner
        // invalidates its publication and starts a new native continuity interval.
        unsafe {
            mixrelay_macos_transport::callback_clock::mr_callback_clock_record_values(
                ptr::from_ref(&self.clock).cast_mut(),
                0.0,
                0,
                false,
                0,
            );
            mixrelay_macos_transport::callback_clock::mr_callback_clock_reset(
                ptr::from_ref(&self.clock).cast_mut(),
            );
        }
    }
}
