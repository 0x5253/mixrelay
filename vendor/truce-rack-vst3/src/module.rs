//! Keep native module entry, factory access, and exit in one loaded-module owner.

use std::path::Path;
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use truce_rack_core::error::{Error, Result};
use vst3::{ComPtr, Steinberg::IPluginFactory};

/// Symbol name `GetPluginFactory` plugins export. Same on every OS.
const GET_FACTORY_SYMBOL: &[u8] = b"GetPluginFactory\0";

#[cfg(target_os = "linux")]
const MODULE_ENTRY_SYMBOL: &[u8] = b"ModuleEntry\0";
#[cfg(target_os = "linux")]
const MODULE_EXIT_SYMBOL: &[u8] = b"ModuleExit\0";

#[cfg(target_os = "windows")]
const DLL_ENTRY_SYMBOL: &[u8] = b"InitDll\0";
#[cfg(target_os = "windows")]
const DLL_EXIT_SYMBOL: &[u8] = b"ExitDll\0";

/// macOS-only bundle entry point.
#[cfg(target_os = "macos")]
const BUNDLE_ENTRY_SYMBOL: &[u8] = b"bundleEntry\0";

/// macOS-only bundle exit point.
#[cfg(target_os = "macos")]
const BUNDLE_EXIT_SYMBOL: &[u8] = b"bundleExit\0";

#[cfg(target_os = "macos")]
mod mac {
    //! `CFBundle`-backed loader for VST3 bundles on macOS.
    //!
    //! Plugins like Surge XT Effects call into `CFPlugin` during
    //! `bundleEntry` (specifically `CFPlugInRegisterFactories`,
    //! which lives behind the `AddInstanceForFactory` log line).
    //! Those APIs only work when the dylib was loaded through a
    //! registered `CFBundle` — raw `dlopen` leaves the bundle
    //! unknown to CoreFoundation and the plugin dereferences
    //! garbage. Going through `CFBundleLoadExecutable` gives the
    //! plugin the context it expects.

    use std::path::{Path, PathBuf};

    use core_foundation::base::TCFType;
    use core_foundation::bundle::CFBundle;
    use core_foundation::string::CFString;
    use core_foundation::url::{CFURL, kCFURLPOSIXPathStyle};

    use truce_rack_core::error::{Error, Result};

    pub(super) struct MacBundle {
        bundle: CFBundle,
        path: PathBuf,
    }

    impl MacBundle {
        pub(super) fn open(bundle_path: &Path) -> Result<Self> {
            let path_str = bundle_path.to_str().ok_or_else(|| Error::LoadFailed {
                path: bundle_path.to_path_buf(),
                reason: "bundle path is not valid UTF-8".into(),
            })?;
            let cf_path = CFString::new(path_str);
            // CFURLCreateWithFileSystemPath with `isDirectory = true`
            // is what every reference VST3 host uses on macOS — a
            // bundle URL has to be flagged as a directory or
            // CFBundleCreate silently picks the wrong layout.
            let url = unsafe {
                use core_foundation_sys::base::kCFAllocatorDefault;
                use core_foundation_sys::url::CFURLCreateWithFileSystemPath;
                let raw = CFURLCreateWithFileSystemPath(
                    kCFAllocatorDefault,
                    cf_path.as_concrete_TypeRef(),
                    kCFURLPOSIXPathStyle,
                    1,
                );
                if raw.is_null() {
                    return Err(Error::LoadFailed {
                        path: bundle_path.to_path_buf(),
                        reason: "CFURLCreateWithFileSystemPath returned NULL".into(),
                    });
                }
                CFURL::wrap_under_create_rule(raw)
            };

            let bundle = CFBundle::new(url).ok_or_else(|| Error::LoadFailed {
                path: bundle_path.to_path_buf(),
                reason: "CFBundleCreate returned NULL".into(),
            })?;

            // CFBundleLoadExecutable must run before any function
            // pointer lookup. Returns true on success.
            let loaded = unsafe {
                use core_foundation_sys::bundle::CFBundleLoadExecutable;
                CFBundleLoadExecutable(bundle.as_concrete_TypeRef()) != 0
            };
            if !loaded {
                return Err(Error::LoadFailed {
                    path: bundle_path.to_path_buf(),
                    reason: "CFBundleLoadExecutable failed".into(),
                });
            }

            Ok(Self {
                bundle,
                path: bundle_path.to_path_buf(),
            })
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }

        /// The raw `CFBundleRef`, type-erased to `*mut c_void` so
        /// the call site doesn't need to depend on
        /// `core-foundation-sys`. Hand this to `bundleEntry` —
        /// it's the host's identity to the plugin.
        pub(super) fn raw(&self) -> *mut std::ffi::c_void {
            self.bundle.as_concrete_TypeRef().cast::<std::ffi::c_void>()
        }

        /// Look up an exported symbol. `name` may end in a trailing
        /// NUL (we strip it before handing the text to `CFString`).
        pub(super) unsafe fn function_ptr(&self, name: &[u8]) -> Option<*mut std::ffi::c_void> {
            let name = match name.split_last() {
                Some((0, rest)) => rest,
                _ => name,
            };
            let name_str = std::str::from_utf8(name).ok()?;
            let cf_name = CFString::new(name_str);
            let ptr = unsafe {
                use core_foundation_sys::bundle::CFBundleGetFunctionPointerForName;
                CFBundleGetFunctionPointerForName(
                    self.bundle.as_concrete_TypeRef(),
                    cf_name.as_concrete_TypeRef(),
                )
            };
            if ptr.is_null() {
                None
            } else {
                Some(ptr.cast_mut().cast::<std::ffi::c_void>())
            }
        }
    }

    // SAFETY: CFBundle is reference-counted by CoreFoundation; we
    // hold one owned reference and CoreFoundation itself is
    // thread-safe for read access. The Drop impl releases the
    // CFBundle but deliberately never calls
    // `CFBundleUnloadExecutable` — VST3 plugins leave Objective-C
    // class registrations and runloop callbacks pointing into the
    // dylib, and unloading invalidates them. Same "don't dlclose"
    // discipline truce-loader follows on the plugin side.
    unsafe impl Send for MacBundle {}
}

/// Per-platform VST3 bundle layout. Linux uses
/// `Contents/<arch>-linux/<stem>.so`; Windows uses
/// `Contents/<arch>-win/<stem>.vst3`. macOS goes through `CFBundle`
/// instead (see [`mac::MacBundle`]) so its binary lookup lives
/// there.
#[cfg(not(target_os = "macos"))]
fn bundle_binary_path(bundle: &Path) -> PathBuf {
    let stem = bundle
        .file_stem()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    #[cfg(target_os = "linux")]
    {
        if bundle.is_dir() {
            let arch_dir = format!("{}-linux", std::env::consts::ARCH);
            let mut binary = stem.clone();
            binary.push(".so");
            return bundle.join("Contents").join(arch_dir).join(binary);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if bundle.is_dir() {
            let arch_dir = windows_arch_dir(std::env::consts::ARCH);
            let mut binary = stem.clone();
            binary.push(".vst3");
            return bundle.join("Contents").join(arch_dir).join(binary);
        }
    }
    let _ = stem;
    bundle.to_path_buf()
}

#[cfg(any(target_os = "windows", test))]
fn windows_arch_dir(arch: &str) -> String {
    let arch = if arch == "aarch64" { "arm64" } else { arch };
    format!("{arch}-win")
}

/// RAII wrapper around the loaded module. On macOS this is a real
/// `CFBundle` so the plugin's `bundleEntry` sees the `CFPlugin` /
/// `CFBundleGetIdentifier` context it expects (raw dlopen crashes
/// some bundles — Surge XT Effects calls into `CFPlugin`'s
/// `AddInstanceForFactory` during init). On Linux / Windows the
/// underlying file is a plain dynamic library; `libloading` owns its
/// handle while platform exit runs before unload.
#[cfg(not(target_os = "macos"))]
pub(super) struct LoadedModule {
    library: libloading::Library,
    exit: Option<unsafe extern "system" fn() -> bool>,
}

#[cfg(target_os = "macos")]
pub(super) struct LoadedModule {
    _lifecycle: bundle_entry::BundleEntryGuard,
    bundle: mac::MacBundle,
}

#[cfg(not(target_os = "macos"))]
impl LoadedModule {
    pub(super) unsafe fn open(bundle: &Path) -> Result<Self> {
        let binary = bundle_binary_path(bundle);
        let library =
            unsafe { libloading::Library::new(&binary) }.map_err(|e| Error::LoadFailed {
                path: bundle.to_path_buf(),
                reason: format!("dlopen: {e}"),
            })?;
        #[cfg(target_os = "linux")]
        let (library, raw_handle) = linux_library_handle(library);
        let mut module = Self {
            library,
            exit: None,
        };
        #[cfg(target_os = "linux")]
        module.enter_linux(bundle, raw_handle)?;
        #[cfg(target_os = "windows")]
        module.enter_windows(bundle)?;
        Ok(module)
    }

    #[cfg(target_os = "linux")]
    fn enter_linux(&mut self, bundle: &Path, raw_handle: *mut std::ffi::c_void) -> Result<()> {
        let entry = unsafe { self.library.get::<LinuxEntry>(MODULE_ENTRY_SYMBOL) }
            .ok()
            .map(|symbol| *symbol);
        let exit = unsafe {
            self.library
                .get::<unsafe extern "system" fn() -> bool>(MODULE_EXIT_SYMBOL)
        }
        .ok()
        .map(|symbol| *symbol);
        self.enter_linux_exports(bundle, raw_handle, entry, exit)
    }

    #[cfg(target_os = "linux")]
    fn enter_linux_exports(
        &mut self,
        bundle: &Path,
        raw_handle: *mut std::ffi::c_void,
        entry: Option<LinuxEntry>,
        exit: Option<unsafe extern "system" fn() -> bool>,
    ) -> Result<()> {
        let entry = entry.ok_or_else(|| Error::LoadFailed {
            path: bundle.to_path_buf(),
            reason: "missing mandatory ModuleEntry".into(),
        })?;
        let exit = exit.ok_or_else(|| Error::LoadFailed {
            path: bundle.to_path_buf(),
            reason: "missing mandatory ModuleExit".into(),
        })?;
        // Keep the exit callback armed even if entry rejects after partial setup.
        self.exit = Some(exit);
        if !unsafe { entry(raw_handle) } {
            return Err(Error::LoadFailed {
                path: bundle.to_path_buf(),
                reason: "ModuleEntry rejected the module".into(),
            });
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn enter_windows(&mut self, bundle: &Path) -> Result<()> {
        let entry = unsafe {
            self.library
                .get::<unsafe extern "system" fn() -> bool>(DLL_ENTRY_SYMBOL)
        }
        .ok()
        .map(|symbol| *symbol);
        self.exit = unsafe {
            self.library
                .get::<unsafe extern "system" fn() -> bool>(DLL_EXIT_SYMBOL)
        }
        .ok()
        .map(|symbol| *symbol);
        if entry.is_some_and(|entry| !unsafe { entry() }) {
            return Err(Error::LoadFailed {
                path: bundle.to_path_buf(),
                reason: "InitDll rejected the module".into(),
            });
        }
        Ok(())
    }

    pub(super) fn factory(&self) -> Result<ComPtr<IPluginFactory>> {
        let get_factory: libloading::Symbol<
            '_,
            unsafe extern "system" fn() -> *mut IPluginFactory,
        > = unsafe { self.library.get(GET_FACTORY_SYMBOL) }.map_err(|e| Error::LoadFailed {
            path: PathBuf::new(),
            reason: format!("missing GetPluginFactory: {e}"),
        })?;
        let ptr = unsafe { get_factory() };
        let factory = unsafe { ComPtr::<IPluginFactory>::from_raw(ptr) }.ok_or_else(|| {
            Error::LoadFailed {
                path: PathBuf::new(),
                reason: "GetPluginFactory returned NULL".into(),
            }
        })?;
        Ok(factory)
    }
}

#[cfg(target_os = "linux")]
type LinuxEntry = unsafe extern "system" fn(*mut std::ffi::c_void) -> bool;

#[cfg(target_os = "linux")]
fn linux_library_handle(
    library: libloading::Library,
) -> (libloading::Library, *mut std::ffi::c_void) {
    let native: libloading::os::unix::Library = library.into();
    let raw_handle = native.into_raw();
    // SAFETY: `raw_handle` came from this exact owned Library; immediately
    // reconstructing it transfers the single dlclose responsibility back.
    let library = unsafe { libloading::os::unix::Library::from_raw(raw_handle) };
    (library.into(), raw_handle)
}

#[cfg(not(target_os = "macos"))]
impl Drop for LoadedModule {
    fn drop(&mut self) {
        if let Some(exit) = self.exit {
            // SAFETY: the callback came from `self.library`, which remains loaded
            // until this Drop body returns and its field is dropped afterward.
            unsafe { exit() };
        }
    }
}

#[cfg(target_os = "macos")]
impl LoadedModule {
    pub(super) unsafe fn open(bundle: &Path) -> Result<Self> {
        let mac_bundle = mac::MacBundle::open(bundle)?;

        // SAFETY: exported functions use the declared VST3 ABI and the loaded
        // CFBundle remains owned until after the entry guard exits.
        let lifecycle = unsafe {
            let entry = mac_bundle.function_ptr(BUNDLE_ENTRY_SYMBOL).map(|pointer| {
                std::mem::transmute::<*mut std::ffi::c_void, bundle_entry::BundleEntry>(pointer)
            });
            let exit = mac_bundle.function_ptr(BUNDLE_EXIT_SYMBOL).map(|pointer| {
                std::mem::transmute::<*mut std::ffi::c_void, bundle_entry::BundleExit>(pointer)
            });
            bundle_entry::BundleEntryGuard::enter(mac_bundle.path(), mac_bundle.raw(), entry, exit)?
        };
        Ok(Self {
            _lifecycle: lifecycle,
            bundle: mac_bundle,
        })
    }

    pub(super) fn factory(&self) -> Result<ComPtr<IPluginFactory>> {
        let raw = unsafe {
            self.bundle
                .function_ptr(GET_FACTORY_SYMBOL)
                .ok_or_else(|| Error::LoadFailed {
                    path: self.bundle.path().to_path_buf(),
                    reason: "missing GetPluginFactory".into(),
                })?
        };
        let get_factory: unsafe extern "C" fn() -> *mut IPluginFactory =
            unsafe { std::mem::transmute(raw) };
        let ptr = unsafe { get_factory() };
        let factory = unsafe { ComPtr::<IPluginFactory>::from_raw(ptr) }.ok_or_else(|| {
            Error::LoadFailed {
                path: self.bundle.path().to_path_buf(),
                reason: "GetPluginFactory returned NULL".into(),
            }
        })?;
        Ok(factory)
    }
}

#[cfg(test)]
mod tests;

#[cfg(any(target_os = "macos", test))]
mod bundle_entry;
