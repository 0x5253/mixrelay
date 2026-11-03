use pipewire::{context::ContextRc, core::CoreRc, thread_loop::ThreadLoopRc};
use std::{
    ffi::{CStr, c_void},
    marker::PhantomData,
};

/// Failed control-side connection preparation. No partially prepared owner escapes.
#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    /// A loop, context or core could not be created; completed stages are reclaimed.
    #[error("PipeWire {stage}: {source}")]
    Create {
        stage: &'static str,
        source: pipewire::Error,
    },
    /// Native control-loop startup failed with the original negative errno.
    #[error("PipeWire control-loop start failed: {0}")]
    Start(i32),
    /// A running loop cannot be started a second time.
    #[error("PipeWire connection already started")]
    AlreadyStarted,
    /// The linked library did not report a numeric three-component version.
    #[error("PipeWire library version is not a supported version string")]
    LibraryVersion,
    /// Async graph contributions require the scheduling contract introduced in 1.2.
    #[error("PipeWire 1.2.0 or newer is required (linked {0})")]
    LibraryTooOld(String),
    /// Inactive capture preparation failed before any DSP or stream activation.
    #[error("PipeWire capture preparation: {0}")]
    Capture(String),
}

fn audio_config(version: &str) -> Result<&'static str, ConnectionError> {
    let mut parts = version.split('.');
    let mut numbers = [0u32; 3];
    for number in &mut numbers {
        *number = parts
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or(ConnectionError::LibraryVersion)?;
    }
    if parts.next().is_some() {
        return Err(ConnectionError::LibraryVersion);
    }
    if numbers < [1, 2, 0] {
        return Err(ConnectionError::LibraryTooOld(version.into()));
    }
    // 1.3.81 merged the audio client configuration into client.conf. Select
    // using the linked library, not build headers: distributions can upgrade it
    // without rebuilding MixRelay. Older supported releases still need -rt.
    Ok(if numbers >= [1, 3, 81] {
        "client.conf"
    } else {
        "client-rt.conf"
    })
}

/// Borrowed ABI handles, valid only while their unique connection remains live.
/// Fields are private to Rust; C receives the corresponding opaque pointer pair.
#[repr(C)]
pub struct ConnectionHandles<'a> {
    core: *mut c_void,
    thread_loop: *mut c_void,
    lifetime: PhantomData<&'a Connection>,
}

/// Unique control-side owner of one native connection and its data/control loops.
/// No `PipeWire` Rust handle or reference-counted clone is exposed. Destroy all
/// attached streams/filters before dropping this owner; see [`Self::handles`].
pub struct Connection {
    // CoreRc retains its ContextRc, which retains the loop. After stopping the
    // control loop, field order disconnects core, destroys context/data loop,
    // then releases the final loop owner. No Rc mutation happens in callbacks.
    core: CoreRc,
    thread_loop: ThreadLoopRc,
    started: bool,
}

// SAFETY: every upstream Rc clone is confined to this unique owner and its
// nested core/context. Moving the entire owner cannot race Rc clone/drop. No
// Rust callback borrows these handles; native operations use the loop lock.
// Connection is deliberately not Sync or Clone and is never retired on RT.
unsafe impl Send for Connection {}

impl Connection {
    /// Completes manager DSP negotiation and prepares this owned capture's
    /// monitor ports while its stream is still inactive. The caller retains the
    /// exact native stream and activates it only after successful completion.
    /// `node_id` identifies that retained stream, not a name/default lookup.
    /// `rate` is its existing RAW PCM rate, used only to encode a positive DSP
    /// `PortConfig` write rate; DSP readback remains graph-clock-selected (zero).
    /// # Errors
    /// Returns configuration, readback or bounded preparation failure. No
    /// stream is activated and temporary node listeners/proxies are reclaimed.
    pub fn prepare_capture(&mut self, node_id: u32, rate: u32) -> Result<(), ConnectionError> {
        if !self.started {
            return Err(ConnectionError::Capture(
                "control loop is not started".into(),
            ));
        }
        crate::capture_control::prepare(self, node_id, rate).map_err(ConnectionError::Capture)
    }

    pub(crate) fn core(&self) -> &pipewire::core::Core {
        &self.core
    }

    pub(crate) fn lock(&self) -> pipewire::thread_loop::ThreadLoopLockGuard<'_> {
        self.thread_loop.lock()
    }

    /// Prepares an unstarted connection using the installed audio client configuration.
    /// `name` labels the control thread; the native API copies it during creation.
    /// # Errors
    /// Returns the failed creation stage. Previously allocated stages drop in order.
    pub fn new(name: &CStr) -> Result<Self, ConnectionError> {
        // SAFETY: pw_get_library_version returns the library's immutable,
        // process-lifetime, NUL-terminated version string. No borrowed owner or
        // native callback participates in this control-side query.
        let version = unsafe { CStr::from_ptr(pipewire::sys::pw_get_library_version()) };
        let config = audio_config(
            version
                .to_str()
                .map_err(|_| ConnectionError::LibraryVersion)?,
        )?;
        // SAFETY: this new unstarted loop has one control-side owner. Nothing
        // escapes until context/core preparation succeeds; no callback is added.
        let thread_loop =
            unsafe { ThreadLoopRc::new_cstr(Some(name), None) }.map_err(|source| {
                ConnectionError::Create {
                    stage: "loop",
                    source,
                }
            })?;
        let context = ContextRc::new(
            &thread_loop,
            Some(pipewire::properties::properties! {
                "config.name" => config,
            }),
        )
        .map_err(|source| ConnectionError::Create {
            stage: "audio context",
            source,
        })?;
        let core = context
            .connect_rc(None)
            .map_err(|source| ConnectionError::Create {
                stage: "core",
                source,
            })?;
        Ok(Self {
            core,
            thread_loop,
            started: false,
        })
    }

    /// Supplies borrowed native handles for stream/filter construction.
    /// # Safety
    /// The native owner may retain these pointers only until its explicit close.
    /// Close must disconnect/drain every child before this connection drops.
    /// All native control operations after start must take this thread-loop lock;
    /// RT callbacks may use only `PipeWire`'s RT-safe operations. The engine pins
    /// this owner next to the child and closes the child in its Drop body first.
    #[must_use]
    pub unsafe fn handles(&self) -> ConnectionHandles<'_> {
        ConnectionHandles {
            core: self.core.as_raw_ptr().cast(),
            thread_loop: self.thread_loop.as_raw_ptr().cast(),
            lifetime: PhantomData,
        }
    }

    /// Starts event dispatch after child preparation, preserving startup errors.
    /// # Errors
    /// Returns the native negative errno or rejects repeated startup.
    pub fn start(&mut self) -> Result<(), ConnectionError> {
        if self.started {
            return Err(ConnectionError::AlreadyStarted);
        }
        // SAFETY: unique unstarted owner, and the control-loop lock is not held.
        // Upstream ThreadLoop::start discards this error, so use the same binding
        // directly to keep failed startup observable and reclaimable.
        let result = unsafe { pipewire::sys::pw_thread_loop_start(self.thread_loop.as_raw_ptr()) };
        if result < 0 {
            return Err(ConnectionError::Start(result));
        }
        self.started = true;
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if self.started {
            self.thread_loop.stop();
        }
        // Rc field destructors now run with the control loop stopped. Child
        // owners have already disconnected and drained all data callbacks.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_configuration_follows_the_linked_library_contract() {
        for version in ["1.2.0", "1.2.7", "1.3.80"] {
            assert_eq!(audio_config(version).unwrap(), "client-rt.conf");
        }
        for version in ["1.3.81", "1.4.2", "2.0.0"] {
            assert_eq!(audio_config(version).unwrap(), "client.conf");
        }
        for version in ["0.3.65", "1.0.5", "1.1.99"] {
            assert!(matches!(
                audio_config(version),
                Err(ConnectionError::LibraryTooOld(_))
            ));
        }
        for version in ["", "1.4", "1.4.2.0", "1.x.0"] {
            assert!(audio_config(version).is_err());
        }
    }

    #[test]
    fn handle_layout_matches_two_native_pointers() {
        assert_eq!(
            std::mem::size_of::<ConnectionHandles<'_>>(),
            2 * std::mem::size_of::<*mut c_void>()
        );
        assert_eq!(
            std::mem::align_of::<ConnectionHandles<'_>>(),
            std::mem::align_of::<*mut c_void>()
        );
    }

    #[cfg(feature = "native-lab")]
    fn resources() -> (usize, usize) {
        (
            std::fs::read_dir("/proc/self/fd").unwrap().count(),
            std::fs::read_dir("/proc/self/task").unwrap().count(),
        )
    }

    #[test]
    #[cfg(feature = "native-lab")]
    fn native_pipewire_connection_lifecycle() {
        assert!(std::env::var_os("MIXRELAY_LAB_APPLICATION_FIXTURE").is_some());
        // Warm library initialization once; compare retained process resources,
        // not allocator/DSO caches against an uninitialized process.
        for _ in 0..2 {
            let mut owner = Connection::new(c"connection-warmup").unwrap();
            owner.start().unwrap();
        }
        let before = resources();
        for _ in 0..16 {
            let mut owner = Connection::new(c"connection-lifecycle").unwrap();
            // SAFETY: this test only observes addresses while their owner lives;
            // no native children or retained pointers are created.
            let handles = unsafe { owner.handles() };
            assert!(!handles.core.is_null() && !handles.thread_loop.is_null());
            owner.start().unwrap();
            assert!(matches!(
                owner.start(),
                Err(ConnectionError::AlreadyStarted)
            ));
        }
        let after = resources();
        assert_eq!(after, before, "FD/thread leak after 16 context cycles");
    }
}
