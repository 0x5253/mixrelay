//! Native listener copies own only the invalid latch, never a borrowed device context.

use std::{
    ffi::c_void,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Release},
    },
};

use block2::{Block, RcBlock};

use super::native::{Address, Properties, StreamIds, System};

const GLOBAL: u32 = u32::from_be_bytes(*b"glob");
type Notification = dyn Fn(u32, *const c_void);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Subscription {
    object: u32,
    address: Address,
}

#[cfg(any(test, feature = "test-observation"))]
impl Subscription {
    /// The watched object and property selector, for a simulated HAL's notifications.
    pub(super) fn target(self) -> (u32, u32) {
        (self.object, self.address.selector)
    }
}

pub(super) struct WatchTarget {
    pub(super) device: u32,
    pub(super) scope: u32,
    pub(super) limit: u32,
    pub(super) unused_scope: Option<u32>,
}

pub(super) trait ListenerBackend {
    fn add(&self, subscription: Subscription, block: &Block<Notification>) -> i32;
    fn remove(&self, subscription: Subscription, block: &Block<Notification>) -> i32;
    fn streams(&self, target: &WatchTarget) -> Option<StreamIds>;
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    #[link_name = "AudioObjectAddPropertyListenerBlock"]
    fn add_listener(
        object: u32,
        address: *const Address,
        queue: *mut c_void,
        block: &Block<Notification>,
    ) -> i32;
    #[link_name = "AudioObjectRemovePropertyListenerBlock"]
    fn remove_listener(
        object: u32,
        address: *const Address,
        queue: *mut c_void,
        block: &Block<Notification>,
    ) -> i32;
}

impl ListenerBackend for System {
    fn add(&self, subscription: Subscription, block: &Block<Notification>) -> i32 {
        // SAFETY: exact SDK address/block ABI. The closure captures only Arc<AtomicBool>,
        // so concurrent native invocations are safe. NULL queue invokes directly instead of
        // adding an asynchronous dispatch delay; CoreAudio Block_copy retains its capture.
        unsafe {
            add_listener(
                subscription.object,
                &raw const subscription.address,
                ptr::null_mut(),
                block,
            )
        }
    }

    fn remove(&self, subscription: Subscription, block: &Block<Notification>) -> i32 {
        // SAFETY: exact registered block identity/address and same NULL queue. CoreAudio releases
        // its copied block; any invocation already retaining it still owns the atomic capture.
        unsafe {
            remove_listener(
                subscription.object,
                &raw const subscription.address,
                ptr::null_mut(),
                block,
            )
        }
    }

    fn streams(&self, target: &WatchTarget) -> Option<StreamIds> {
        Properties::streams(self, target.device, target.scope, target.limit)
    }
}

/// Control-owned removal receipts. Failed removal keeps the exact block and address for retry.
#[derive(Default)]
pub(super) struct Listeners {
    block: Option<RcBlock<Notification>>,
    subscriptions: Vec<Subscription>,
}

impl Listeners {
    /// Watches device rate/layout before querying streams, then watches each stream's codec.
    /// Returns native registration/query errors while retaining any successful partial watches.
    pub(super) fn install(
        &mut self,
        target: &WatchTarget,
        invalid: &Arc<AtomicBool>,
        backend: &impl ListenerBackend,
    ) -> Result<(), i32> {
        if self.block.is_some() {
            return Err(-6);
        }
        let latch = invalid.clone();
        self.block = Some(RcBlock::new(move |_: u32, _: *const c_void| {
            latch.store(true, Release);
        }));
        // A restarted HAL can recycle numeric object IDs while geometry stays
        // identical. Invalidate before rediscovering any device/stream identity.
        self.subscribe(
            Subscription {
                object: 1,
                address: Address::new(u32::from_be_bytes(*b"srst"), GLOBAL),
            },
            backend,
        )?;
        for (selector, scope) in [
            (*b"nsrt", GLOBAL),
            (*b"stm#", target.scope),
            (*b"slay", target.scope),
            // Device removal must silence an already registered callback before polling.
            (*b"livn", GLOBAL),
        ] {
            self.subscribe(
                Subscription {
                    object: target.device,
                    address: Address::new(u32::from_be_bytes(selector), scope),
                },
                backend,
            )?;
        }
        if let Some(scope) = target.unused_scope {
            // New unused streams must be disabled before this IOProc restarts.
            self.subscribe(
                Subscription {
                    object: target.device,
                    address: Address::new(u32::from_be_bytes(*b"stm#"), scope),
                },
                backend,
            )?;
        }
        let streams = backend.streams(target).ok_or(-3)?;
        for &stream in &streams.values[..streams.count] {
            for selector in [*b"sfmt", *b"pft ", *b"schn"] {
                self.subscribe(
                    Subscription {
                        object: stream,
                        address: Address::new(u32::from_be_bytes(selector), GLOBAL),
                    },
                    backend,
                )?;
            }
        }
        Ok(())
    }

    fn subscribe(
        &mut self,
        subscription: Subscription,
        backend: &impl ListenerBackend,
    ) -> Result<(), i32> {
        let Some(block) = &self.block else {
            return Err(-6);
        };
        // Allocate the receipt before native add can retain the block. OOM cannot lose a watch.
        self.subscriptions.try_reserve(1).map_err(|_| -6)?;
        let status = backend.add(subscription, block);
        if status != 0 {
            return Err(status);
        }
        self.subscriptions.push(subscription);
        Ok(())
    }

    /// Removes all native watches, retaining failed identities. Deleted objects acknowledge release.
    /// `true` permits owner teardown; delayed copied blocks do not refer to that owner.
    pub(super) fn remove(&mut self, backend: &impl ListenerBackend) -> bool {
        if let Some(block) = &self.block {
            self.subscriptions.retain(|&subscription| {
                !matches!(
                    backend.remove(subscription, block),
                    0 | BAD_OBJECT | BAD_DEVICE
                )
            });
        }
        if self.subscriptions.is_empty() {
            self.block = None;
            return true;
        }
        false
    }
}

const BAD_OBJECT: i32 = i32::from_be_bytes(*b"!obj");
const BAD_DEVICE: i32 = i32::from_be_bytes(*b"!dev");

#[cfg(test)]
mod tests;
