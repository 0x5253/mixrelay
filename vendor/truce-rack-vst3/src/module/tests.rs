//! Behavioral fixtures for the adjacent native owner.

use super::*;

#[test]
fn windows_arm64_bundle_directory_matches_the_sdk() {
    assert_eq!(windows_arch_dir("aarch64"), "arm64-win");
    assert_eq!(windows_arch_dir("x86_64"), "x86_64-win");
    assert_eq!(windows_arch_dir("x86"), "x86-win");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_module_requires_both_hooks_and_exits_after_entry() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static ENTRY_CALLS: AtomicUsize = AtomicUsize::new(0);
    static EXIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static LAST_HANDLE: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn accepted(handle: *mut std::ffi::c_void) -> bool {
        LAST_HANDLE.store(handle as usize, Ordering::SeqCst);
        ENTRY_CALLS.fetch_add(1, Ordering::SeqCst);
        true
    }
    unsafe extern "system" fn rejected(handle: *mut std::ffi::c_void) -> bool {
        LAST_HANDLE.store(handle as usize, Ordering::SeqCst);
        ENTRY_CALLS.fetch_add(1, Ordering::SeqCst);
        false
    }
    unsafe extern "system" fn exit() -> bool {
        EXIT_CALLS.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn module() -> LoadedModule {
        LoadedModule {
            library: libloading::os::unix::Library::this().into(),
            exit: None,
        }
    }

    ENTRY_CALLS.store(0, Ordering::SeqCst);
    EXIT_CALLS.store(0, Ordering::SeqCst);
    let bundle = Path::new("test.vst3");
    let handle = 0x1234usize as *mut std::ffi::c_void;
    let mut missing = module();
    assert!(
        missing
            .enter_linux_exports(bundle, handle, None, Some(exit))
            .expect_err("ModuleEntry is mandatory")
            .to_string()
            .contains("ModuleEntry")
    );
    assert!(
        missing
            .enter_linux_exports(bundle, handle, Some(accepted), None)
            .expect_err("ModuleExit is mandatory")
            .to_string()
            .contains("ModuleExit")
    );
    drop(missing);
    assert_eq!(ENTRY_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(EXIT_CALLS.load(Ordering::SeqCst), 0);

    let mut denied = module();
    assert!(
        denied
            .enter_linux_exports(bundle, handle, Some(rejected), Some(exit))
            .expect_err("rejected entry fails loading")
            .to_string()
            .contains("ModuleEntry rejected")
    );
    drop(denied);
    assert_eq!(ENTRY_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(EXIT_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(LAST_HANDLE.load(Ordering::SeqCst), handle as usize);

    let mut loaded = module();
    loaded
        .enter_linux_exports(bundle, handle, Some(accepted), Some(exit))
        .expect("accepted entry loads");
    drop(loaded);
    assert_eq!(ENTRY_CALLS.load(Ordering::SeqCst), 2);
    assert_eq!(EXIT_CALLS.load(Ordering::SeqCst), 2);
}
