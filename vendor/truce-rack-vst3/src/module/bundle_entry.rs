//! Own the symmetric macOS module entry and exit calls.

use std::{ffi::c_void, path::Path};
use truce_rack_core::error::{Error, Result};

pub(super) type BundleEntry = unsafe extern "C" fn(*mut c_void) -> bool;
pub(super) type BundleExit = unsafe extern "C" fn() -> bool;

pub(super) struct BundleEntryGuard {
    exit: Option<BundleExit>,
}
impl BundleEntryGuard {
    /// # Safety
    /// The callbacks must be native VST3 exports whose module and `CFBundle`
    /// context remain alive until this guard is dropped.
    pub(super) unsafe fn enter(
        path: &Path,
        context: *mut c_void,
        entry: Option<BundleEntry>,
        exit: Option<BundleExit>,
    ) -> Result<Self> {
        let entry = entry.ok_or_else(|| Error::LoadFailed {
            path: path.to_path_buf(),
            reason: "missing mandatory bundleEntry".into(),
        })?;
        let exit = exit.ok_or_else(|| Error::LoadFailed {
            path: path.to_path_buf(),
            reason: "missing mandatory bundleExit".into(),
        })?;
        let guard = Self { exit: Some(exit) };
        // SAFETY: caller retains the native module and its CFBundle context.
        if !unsafe { entry(context) } {
            return Err(Error::LoadFailed {
                path: path.to_path_buf(),
                reason: "bundleEntry rejected the module".into(),
            });
        }
        Ok(guard)
    }
}
impl Drop for BundleEntryGuard {
    fn drop(&mut self) {
        if let Some(exit) = self.exit {
            // SAFETY: the loaded module outlives this guard.
            unsafe {
                exit();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ENTERS: AtomicUsize = AtomicUsize::new(0);
    static EXITS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn accepted(_: *mut c_void) -> bool {
        ENTERS.fetch_add(1, Ordering::Relaxed);
        true
    }
    unsafe extern "C" fn rejected(_: *mut c_void) -> bool {
        ENTERS.fetch_add(1, Ordering::Relaxed);
        false
    }
    unsafe extern "C" fn exit() -> bool {
        EXITS.fetch_add(1, Ordering::Relaxed);
        true
    }
    #[test]
    fn mandatory_exports_and_rejected_entry_prevent_factory_and_clean_partial_entry() {
        let path = Path::new("fixture.vst3");
        let context = std::ptr::null_mut();
        for (entry, exit) in [
            (None, Some(exit as BundleExit)),
            (Some(accepted as BundleEntry), None),
        ] {
            assert!(
                unsafe { BundleEntryGuard::enter(path, context, entry, exit) }.is_err(),
                "incomplete mandatory module hooks reached factory setup"
            );
        }
        assert_eq!(ENTERS.load(Ordering::Relaxed), 0);
        assert_eq!(EXITS.load(Ordering::Relaxed), 0);
        assert!(
            unsafe { BundleEntryGuard::enter(path, context, Some(rejected), Some(exit)) }.is_err()
        );
        assert_eq!(ENTERS.load(Ordering::Relaxed), 1);
        assert_eq!(EXITS.load(Ordering::Relaxed), 1);
        let guard =
            unsafe { BundleEntryGuard::enter(path, context, Some(accepted), Some(exit)) }.unwrap();
        assert_eq!(ENTERS.load(Ordering::Relaxed), 2);
        assert_eq!(EXITS.load(Ordering::Relaxed), 1);
        drop(guard);
        assert_eq!(EXITS.load(Ordering::Relaxed), 2);
    }
}
