//! Registered client ownership and first-start/final-stop native transitions.
use super::Registry;
use crate::{
    abi::{CAPACITY, INVALID, NATIVE_ERROR, NOT_FOUND, OK, RUNNING},
    device::{Control, Device, lock},
};
use std::sync::atomic::Ordering;

pub(crate) enum ClientIdentity {
    Register { pid: i32, bundle: Option<String> },
    Remove,
}

impl Registry {
    pub(crate) fn client_identity(
        &self,
        device: u32,
        client: u32,
        identity: ClientIdentity,
    ) -> i32 {
        let add = matches!(identity, ClientIdentity::Register { .. });
        let _gate = lock(&self.client_identity_gate);
        let status = self.client(device, client, add);
        if status == OK {
            if let ClientIdentity::Register { pid, bundle } = identity {
                self.client_pid(device, client, u32::try_from(pid).unwrap_or(0));
                self.transport.client(device, client, pid, bundle);
            } else {
                self.transport.remove(device, client);
            }
        }
        status
    }

    pub(crate) fn client(&self, id: u32, client: u32, add: bool) -> i32 {
        let Some(device) = self.pin(id).filter(|device| device.id == id) else {
            return NOT_FOUND;
        };
        let mut control = lock(&device.control);
        let changed = if add {
            self.add_client(&device, &mut control, client)
        } else {
            self.remove_client(&device, &mut control, client)
        };
        // The freeze has ended: release any queue the change retired.
        // SAFETY: control retains the live runtime; IO callbacks never read retired storage.
        if device.alive.load(Ordering::Acquire) {
            unsafe { (self.native.release)(device.runtime.load(Ordering::Acquire)) };
        }
        let stopped = match changed {
            Ok(stopped) => stopped,
            Err(status) => return status,
        };
        drop(control);
        drop(device);
        if stopped {
            self.host.changed(id, RUNNING);
        }
        if !add {
            self.reclaim();
        }
        if let Some(signal) = self.control_signal.get() {
            signal.notify();
        }
        OK
    }

    fn add_client(&self, device: &Device, control: &mut Control, client: u32) -> Result<bool, i32> {
        if !device.alive.load(Ordering::Acquire) {
            return Err(NOT_FOUND);
        }
        if control.clients.contains_key(&client) {
            return Err(INVALID);
        }
        let runtime = device.runtime.load(Ordering::Acquire);
        // A client added while another holds device IO may render before its own StartIO,
        // so it gets its queue now; otherwise its first StartIO maps one.
        let prepared = if device.running.load(Ordering::Acquire) == 0 {
            None
        } else {
            // SAFETY: control retains this live runtime. Mapping the queue runs alongside
            // IO callbacks; only its publication needs the freeze.
            unsafe { self.native.prepare_queue(runtime, client) }?
        };
        let _frozen = device.freeze();
        // SAFETY: control and freeze retain this live runtime and exclude IO readers.
        if !unsafe {
            self.native.update_client(
                runtime,
                client,
                crate::io_outcome::ClientChange::Register,
                prepared,
            )
        } {
            return Err(NATIVE_ERROR);
        }
        control.clients.insert(client, false);
        device.input_latency.add(client);
        Ok(false)
    }

    fn remove_client(
        &self,
        device: &Device,
        control: &mut Control,
        client: u32,
    ) -> Result<bool, i32> {
        let frozen = device.freeze();
        let stopped = match control.clients.get(&client) {
            Some(false) => false,
            Some(true) => {
                // A client process can disappear without a balanced StopIO.
                // Its removal releases only that client's runtime claim.
                let count = device.running.load(Ordering::Acquire) - 1;
                if device.alive.load(Ordering::Acquire) {
                    let runtime = device.runtime.load(Ordering::Acquire);
                    if count == 0 {
                        // SAFETY: control and freeze retain the live, drained runtime.
                        unsafe { (self.native.stop)(runtime) };
                    } else {
                        // SAFETY: the same drained runtime retains the other clients.
                        unsafe { (self.native.remaining_client)(runtime) };
                    }
                }
                device.running.store(count, Ordering::Release);
                count == 0
            }
            None => return Err(INVALID),
        };
        control.clients.remove(&client);
        if device.alive.load(Ordering::Acquire) {
            // SAFETY: control and freeze exclude callbacks during queue retirement.
            unsafe {
                self.native.update_client(
                    device.runtime.load(Ordering::Acquire),
                    client,
                    crate::io_outcome::ClientChange::Remove,
                    None,
                )
            };
        }
        device.input_latency.remove(client);
        drop(frozen);
        Ok(stopped)
    }

    /// Maps a queue for each client holding IO on `id` that lacks one; see `perform`.
    pub(crate) fn reprovision(&self, id: u32) {
        if let Some(device) = self.pin(id).filter(|device| device.id == id) {
            self.provision_running(&device, &mut lock(&device.control));
        }
    }

    /// Queues map on control outside any freeze, while the host still pauses IO; only
    /// their publication takes one short freeze. A client whose queue could not be
    /// mapped or installed stays counted in `unprovisioned` for the control tick.
    pub(super) fn provision_running(&self, device: &Device, control: &mut Control) {
        if !device.alive.load(Ordering::Acquire) {
            return;
        }
        let runtime = device.runtime.load(Ordering::Acquire);
        let mut missing = 0;
        let prepared: Vec<_> = control
            .clients
            .iter()
            .filter(|&(_, &running)| running)
            // SAFETY: control retains the live runtime; mapping runs alongside IO.
            .filter_map(|(&client, _)| {
                let prepared = unsafe { self.native.prepare_queue(runtime, client) };
                missing += u32::from(prepared.is_err());
                prepared.ok().flatten().map(|prepared| (client, prepared))
            })
            .collect();
        if !prepared.is_empty() {
            let frozen = device.freeze();
            for (client, prepared) in prepared {
                // SAFETY: control and freeze retain the live runtime and exclude IO.
                let installed = unsafe {
                    self.native.update_client(
                        runtime,
                        client,
                        crate::io_outcome::ClientChange::Install,
                        Some(prepared),
                    )
                };
                missing += u32::from(!installed);
            }
            drop(frozen);
            // SAFETY: control retains the live runtime; the freeze has ended.
            unsafe { (self.native.release)(runtime) };
        }
        control.unprovisioned = missing;
    }

    /// Running clients still without a queue, per endpoint, for the Driver service.
    pub(crate) fn unprovisioned(&self) -> Vec<([u8; 16], u32)> {
        self.devices()
            .into_iter()
            .filter_map(|id| {
                let device = self.pin(id)?;
                let missing = lock(&device.control).unprovisioned;
                (missing != 0).then_some((device.endpoint, missing))
            })
            .collect()
    }

    pub(crate) fn start(&self, id: u32, client: u32, now: u64) -> i32 {
        let Some(device) = self.pin(id).filter(|device| device.id == id) else {
            return NOT_FOUND;
        };
        let mut control = lock(&device.control);
        if !device.alive.load(Ordering::Acquire) {
            return NOT_FOUND;
        }
        if control.clients.get(&client) != Some(&false) {
            return INVALID;
        }
        let count = device.running.load(Ordering::Acquire);
        if count == u32::MAX {
            return CAPACITY;
        }
        let runtime = device.runtime.load(Ordering::Acquire);
        // A client without a queue of the current shape, or whose ring has used every
        // epoch, maps a fresh one here: on control, outside the freeze, and only for a
        // client that actually starts IO.
        // SAFETY: control retains the live runtime; mapping runs alongside IO callbacks.
        let prepared = match unsafe { self.native.prepare_queue(runtime, client) } {
            Ok(prepared) => prepared,
            Err(status) => return status,
        };
        let frozen = device.freeze();
        // SAFETY: control and freeze retain the live runtime and exclude IO readers.
        let started = unsafe {
            self.native.update_client(
                runtime,
                client,
                crate::io_outcome::ClientChange::Install,
                prepared,
            )
        } && (count != 0 || unsafe { (self.native.start)(runtime, now) });
        if started {
            control.clients.insert(client, true);
            device.input_latency.active(client, true);
            device.running.store(count + 1, Ordering::Release);
        }
        drop(frozen);
        // SAFETY: control retains the live runtime; the freeze has ended.
        unsafe { (self.native.release)(runtime) };
        if !started {
            return NATIVE_ERROR;
        }
        drop(control);
        drop(device);
        if count == 0 {
            self.host.changed(id, RUNNING);
            if let Some(signal) = self.control_signal.get() {
                signal.notify();
            }
        }
        OK
    }

    pub(crate) fn stop(&self, id: u32, client: u32) -> i32 {
        let Some(device) = self.pin(id).filter(|device| device.id == id) else {
            return NOT_FOUND;
        };
        let mut control = lock(&device.control);
        let Some(running) = control.clients.get_mut(&client) else {
            return INVALID;
        };
        if !*running {
            return INVALID;
        }
        let frozen = device.freeze();
        let count = device.running.load(Ordering::Acquire) - 1;
        if device.alive.load(Ordering::Acquire) {
            let runtime = device.runtime.load(Ordering::Acquire);
            if count == 0 {
                // SAFETY: control and freeze retain the live, drained runtime.
                unsafe { (self.native.stop)(runtime) };
            } else {
                // SAFETY: the same drained runtime retains the other clients.
                unsafe { (self.native.remaining_client)(runtime) };
            }
        }
        *running = false;
        device.input_latency.active(client, false);
        device.running.store(count, Ordering::Release);
        drop(frozen);
        drop(control);
        drop(device);
        if count == 0 {
            self.host.changed(id, RUNNING);
        }
        OK
    }
}
