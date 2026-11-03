//! Loaded CLAP entries and partial-initialization guards.

use crate::ENTRY_SYMBOL;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::plugin::clap_plugin;
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use truce_rack_core::error::{Error, Result};

/// RAII wrapper around a `libloading::Library` plus the
/// `clap_entry` symbol it exports.
pub(super) struct LoadedLibrary {
    pub(super) library: libloading::Library,
}

impl LoadedLibrary {
    pub(super) unsafe fn open(path: &Path) -> Result<Self> {
        // SAFETY: caller guarantees `path` points at a valid
        // CLAP plugin bundle binary; `libloading::Library::new`
        // is itself the OS dlopen call.
        let library = unsafe { libloading::Library::new(path) }.map_err(|e| Error::LoadFailed {
            path: path.to_path_buf(),
            reason: format!("dlopen failed: {e}"),
        })?;
        Ok(Self { library })
    }

    pub(super) fn entry(&self) -> Result<EntryRef<'_>> {
        let symbol: libloading::Symbol<'_, *const clap_plugin_entry> = unsafe {
            self.library
                .get(ENTRY_SYMBOL)
                .map_err(|e| Error::LoadFailed {
                    path: PathBuf::new(),
                    reason: format!("missing clap_entry symbol: {e}"),
                })?
        };
        let entry = *symbol;
        if entry.is_null() {
            return Err(Error::LoadFailed {
                path: PathBuf::new(),
                reason: "clap_entry symbol resolved to NULL".into(),
            });
        }
        Ok(EntryRef {
            entry,
            _phantom: std::marker::PhantomData,
        })
    }
}

pub(super) struct EntryRef<'a> {
    pub(super) entry: *const clap_plugin_entry,
    // The reference lifetime ties the entry pointer to the
    // library handle it was loaded from.
    pub(super) _phantom: std::marker::PhantomData<&'a LoadedLibrary>,
}

impl EntryRef<'_> {
    pub(super) unsafe fn init(&self, path: &Path) -> Result<()> {
        let init = unsafe { (*self.entry).init };
        if let Some(init) = init {
            let c_path =
                CString::new(path.to_string_lossy().as_bytes()).map_err(|e| Error::LoadFailed {
                    path: path.to_path_buf(),
                    reason: format!("plugin path is not a valid C string: {e}"),
                })?;
            if !unsafe { init(c_path.as_ptr()) } {
                return Err(Error::LoadFailed {
                    path: path.to_path_buf(),
                    reason: "clap_entry::init returned false".into(),
                });
            }
        }
        Ok(())
    }

    pub(super) unsafe fn deinit(&self) {
        if let Some(deinit) = unsafe { (*self.entry).deinit } {
            unsafe { deinit() };
        }
    }

    pub(super) unsafe fn factory(&self) -> *const clap_plugin_factory {
        let Some(get_factory) = (unsafe { (*self.entry).get_factory }) else {
            return ptr::null();
        };
        let raw = unsafe { get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) };
        raw.cast::<clap_plugin_factory>()
    }
}

pub(super) struct EntryInitGuard {
    pub(super) entry: *const clap_plugin_entry,
    pub(super) armed: bool,
    pub(super) _not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl EntryInitGuard {
    pub(super) fn new(entry: *const clap_plugin_entry) -> Self {
        Self {
            entry,
            armed: true,
            _not_send: std::marker::PhantomData,
        }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for EntryInitGuard {
    fn drop(&mut self) {
        if self.armed
            && let Some(deinit) = unsafe { (*self.entry).deinit }
        {
            // SAFETY: the load path creates this guard only after entry.init succeeds and keeps
            // the owning LoadedLibrary declared before the guard until it is dropped.
            unsafe { deinit() };
        }
    }
}

pub(super) struct PluginInstanceGuard(
    *const clap_plugin,
    std::marker::PhantomData<std::rc::Rc<()>>,
);

impl PluginInstanceGuard {
    pub(super) fn new() -> Self {
        Self(ptr::null(), std::marker::PhantomData)
    }

    pub(super) fn set(&mut self, plugin: *const clap_plugin) {
        self.0 = plugin;
    }

    pub(super) fn take(&mut self) -> *const clap_plugin {
        let plugin = self.0;
        self.0 = ptr::null();
        plugin
    }
}

impl Drop for PluginInstanceGuard {
    fn drop(&mut self) {
        if !self.0.is_null()
            && let Some(destroy) = unsafe { (*self.0).destroy }
        {
            // SAFETY: the guard owns a plugin returned by the factory and is dropped before its
            // host context, entry state, and library.
            unsafe { destroy(self.0) };
        }
    }
}

/// Look up a CLAP extension by id. Returns `null` if the plugin
/// doesn't implement that extension. The returned pointer's
/// lifetime is the plugin instance; callers must not outlive
/// the plugin.
pub(super) unsafe fn lookup_extension<T>(plugin: *const clap_plugin, id: &CStr) -> *const T {
    let Some(get_extension) = (unsafe { (*plugin).get_extension }) else {
        return ptr::null();
    };
    let raw = unsafe { get_extension(plugin, id.as_ptr()) };
    raw.cast::<T>()
}
