//! Native parent surfaces for third-party processor editors.
//!
//! The plug-in remains in its isolated DSP worker. The desktop/control plane only brokers
//! idempotent open and close requests, while this module owns the native parent window in the
//! same process as the plug-in instance. That is required on macOS (`NSView` pointers cannot
//! cross processes) and avoids unsupported cross-process child-window behavior.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorHostFailure {
    DisplayUnavailable,
    HostWindow,
    PluginRejected,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EditorHostEvents {
    pub close_requested: bool,
    pub resized: Option<(u32, u32)>,
}

impl EditorHostEvents {
    fn merge(&mut self, incoming: Self) {
        self.close_requested |= incoming.close_requested;
        if self.close_requested {
            self.resized = None;
        } else if incoming.resized.is_some() {
            self.resized = incoming.resized;
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn changed_editor_size(current: (u32, u32), requested: Option<(u32, u32)>) -> Option<(u32, u32)> {
    requested.filter(|&(width, height)| width > 0 && height > 0 && current != (width, height))
}

#[cfg(any(target_os = "linux", test))]
fn editor_resize_request(
    observed: (u32, u32),
    requested: Option<(u32, u32)>,
    pending: &mut Option<(u32, u32)>,
) -> Option<(u32, u32)> {
    if requested == Some(observed) {
        *pending = None;
    }
    let size = changed_editor_size(observed, requested)?;
    if *pending == Some(size) {
        return None;
    }
    *pending = Some(size);
    Some(size)
}

#[cfg(target_os = "macos")]
#[path = "native_editor/macos.rs"]
mod platform;

#[cfg(target_os = "linux")]
mod platform {
    use std::{ffi::CString, ptr};

    use truce_rack_core::WindowHandle;
    use x11_dl::xlib;

    use super::{EditorHostEvents, EditorHostFailure};
    use crate::rack_formats::{EditorSnapshot, RackWorkerPlugin};

    const DEFAULT_WIDTH: u32 = 720;
    const DEFAULT_HEIGHT: u32 = 480;

    pub(crate) struct NativeEditorHost {
        xlib: xlib::Xlib,
        display: *mut xlib::Display,
        window: Option<xlib::Window>,
        size: (u32, u32),
        pending_size: Option<(u32, u32)>,
        wm_delete_window: xlib::Atom,
        pub(super) pending_events: EditorHostEvents,
    }

    impl NativeEditorHost {
        pub(crate) fn new(
            _shared: std::sync::Arc<crate::shared_transport::WorkerControl>,
        ) -> Result<Self, EditorHostFailure> {
            if std::env::var_os("DISPLAY").is_none() {
                return Err(EditorHostFailure::DisplayUnavailable);
            }
            let xlib = xlib::Xlib::open().map_err(|_| EditorHostFailure::DisplayUnavailable)?;
            // SAFETY: XOpenDisplay copies DISPLAY and returns a connection closed once in Drop.
            let display = unsafe { (xlib.XOpenDisplay)(ptr::null()) };
            if display.is_null() {
                return Err(EditorHostFailure::DisplayUnavailable);
            }
            // SAFETY: display is live and the atom name is a static NUL-terminated literal.
            let wm_delete_window =
                unsafe { (xlib.XInternAtom)(display, c"WM_DELETE_WINDOW".as_ptr(), xlib::False) };
            Ok(Self {
                xlib,
                display,
                window: None,
                size: (DEFAULT_WIDTH, DEFAULT_HEIGHT),
                pending_size: None,
                wm_delete_window,
                pending_events: EditorHostEvents::default(),
            })
        }

        pub(crate) fn open(
            &mut self,
            plugin: &mut RackWorkerPlugin,
            title: &str,
        ) -> Result<EditorSnapshot, EditorHostFailure> {
            if let Some(window) = self.window {
                // SAFETY: the XID belongs to this live display connection.
                unsafe {
                    (self.xlib.XMapRaised)(self.display, window);
                    (self.xlib.XSetInputFocus)(
                        self.display,
                        window,
                        xlib::RevertToParent,
                        xlib::CurrentTime,
                    );
                    (self.xlib.XFlush)(self.display);
                }
                return Ok(plugin.tick_editor());
            }
            // SAFETY: the display remains live for the full host lifetime.
            let screen = unsafe { (self.xlib.XDefaultScreen)(self.display) };
            let root = unsafe { (self.xlib.XRootWindow)(self.display, screen) };
            let background = unsafe { (self.xlib.XBlackPixel)(self.display, screen) };
            // SAFETY: all arguments are derived from the live display and its default screen.
            let window = unsafe {
                (self.xlib.XCreateSimpleWindow)(
                    self.display,
                    root,
                    0,
                    0,
                    DEFAULT_WIDTH,
                    DEFAULT_HEIGHT,
                    0,
                    background,
                    background,
                )
            };
            if window == 0 {
                return Err(EditorHostFailure::HostWindow);
            }
            self.size = (DEFAULT_WIDTH, DEFAULT_HEIGHT);
            let title = CString::new(title).unwrap_or_else(|_| c"MixRelay plug-in".to_owned());
            let mut protocols = [self.wm_delete_window];
            // SAFETY: the new window is owned by this display. Xlib copies title/protocol data.
            unsafe {
                (self.xlib.XStoreName)(self.display, window, title.as_ptr());
                (self.xlib.XSelectInput)(self.display, window, xlib::StructureNotifyMask);
                (self.xlib.XSetWMProtocols)(self.display, window, protocols.as_mut_ptr(), 1);
                // Vendors may embed through a separate X connection. Confirm that the
                // server created our parent before handing its XID to that connection.
                (self.xlib.XSync)(self.display, xlib::False);
            }
            let snapshot = match plugin.open_editor(WindowHandle::X11(window), 1.0) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(%error, "X11 plug-in editor rejected its parent window");
                    // SAFETY: the window was created above and has not been destroyed.
                    unsafe { (self.xlib.XDestroyWindow)(self.display, window) };
                    return Err(EditorHostFailure::PluginRejected);
                }
            };
            if let Some((width, height)) = snapshot.size {
                // SAFETY: the XID belongs to this display.
                unsafe {
                    (self.xlib.XResizeWindow)(self.display, window, width.max(1), height.max(1))
                };
                self.pending_size = Some((width.max(1), height.max(1)));
            }
            // SAFETY: the XID belongs to this display and XFlush does not retain pointers.
            unsafe {
                (self.xlib.XMapRaised)(self.display, window);
                (self.xlib.XFlush)(self.display);
            }
            self.window = Some(window);
            Ok(snapshot)
        }

        pub(crate) fn close(&mut self, plugin: &mut RackWorkerPlugin) {
            plugin.close_editor();
            self.destroy_window();
        }

        pub(super) fn collect_events(&mut self) -> EditorHostEvents {
            let Some(window) = self.window else {
                return EditorHostEvents::default();
            };
            let mut result = EditorHostEvents::default();
            // SAFETY: events are read only from this host's live display on its owning thread.
            unsafe {
                while (self.xlib.XPending)(self.display) > 0 {
                    let mut event = std::mem::MaybeUninit::<xlib::XEvent>::uninit();
                    (self.xlib.XNextEvent)(self.display, event.as_mut_ptr());
                    let event = event.assume_init();
                    match event.get_type() {
                        xlib::ClientMessage => {
                            let client = xlib::XClientMessageEvent::from(event);
                            if client.window == window
                                && client.format == 32
                                && client.data.get_long(0).cast_unsigned() == self.wm_delete_window
                            {
                                result.close_requested = true;
                            }
                        }
                        xlib::ConfigureNotify => {
                            let configure = xlib::XConfigureEvent::from(event);
                            if configure.window == window {
                                let observed = (
                                    u32::try_from(configure.width).unwrap_or(1).max(1),
                                    u32::try_from(configure.height).unwrap_or(1).max(1),
                                );
                                if let Some(size) =
                                    super::changed_editor_size(self.size, Some(observed))
                                {
                                    result.resized = Some(size);
                                    self.size = size;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            result
        }

        pub(crate) fn refresh(&mut self, snapshot: EditorSnapshot) {
            if !snapshot.open {
                self.destroy_window();
                return;
            }
            if let Some(window) = self.window
                && let Some((width, height)) =
                    super::editor_resize_request(self.size, snapshot.size, &mut self.pending_size)
            {
                // SAFETY: this host owns the live display and XID on the main thread.
                // Dimensions are positive; XResizeWindow and XFlush retain no pointers.
                unsafe {
                    (self.xlib.XResizeWindow)(self.display, window, width, height);
                    (self.xlib.XFlush)(self.display);
                }
            }
        }

        fn destroy_window(&mut self) {
            self.pending_size = None;
            self.pending_events = EditorHostEvents::default();
            if let Some(window) = self.window.take() {
                // SAFETY: this XID belongs to the live display and is destroyed at most once.
                unsafe {
                    (self.xlib.XDestroyWindow)(self.display, window);
                    (self.xlib.XFlush)(self.display);
                }
            }
        }
    }

    impl Drop for NativeEditorHost {
        fn drop(&mut self) {
            self.destroy_window();
            // SAFETY: display was returned by XOpenDisplay and has not been closed yet.
            unsafe { (self.xlib.XCloseDisplay)(self.display) };
        }
    }
}

pub(crate) use platform::NativeEditorHost;

impl NativeEditorHost {
    /// Native tracking loops may block until mouse-up; never hold the DSP mutex here.
    pub(crate) fn poll_events(&mut self) {
        let events = self.collect_events();
        self.pending_events.merge(events);
    }

    /// Consume only after the broker has acquired the plug-in for lifecycle operations.
    pub(crate) fn take_events(&mut self) -> EditorHostEvents {
        std::mem::take(&mut self.pending_events)
    }
}

#[cfg(test)]
mod tests {
    use super::{EditorHostEvents, changed_editor_size, editor_resize_request};

    #[test]
    fn constrained_window_does_not_repeat_an_unfulfilled_resize() {
        let requested = Some((800, 600));
        let constrained = (900, 700);
        let mut pending = None;
        assert_eq!(
            editor_resize_request(constrained, requested, &mut pending),
            requested
        );
        for _ in 0..20 {
            assert_eq!(
                editor_resize_request(constrained, requested, &mut pending),
                None
            );
        }
        // An accepted size clears the request so a later user drag can be corrected.
        assert_eq!(
            editor_resize_request((800, 600), requested, &mut pending),
            None
        );
        assert_eq!(pending, None);
        assert_eq!(
            editor_resize_request(constrained, requested, &mut pending),
            requested
        );
        // A different plugin size is a new request even after an OS constraint.
        let next = Some((1000, 800));
        assert_eq!(editor_resize_request(constrained, next, &mut pending), next);
    }

    #[test]
    fn editor_size_tracks_accepted_pixels_without_repeating_unchanged_requests() {
        let dragged = (900, 700);
        let accepted = (800, 600);
        assert_eq!(changed_editor_size(dragged, Some(accepted)), Some(accepted));
        assert_eq!(changed_editor_size(accepted, Some(accepted)), None);
        assert_eq!(changed_editor_size(accepted, Some(dragged)), Some(dragged));
        assert_eq!(changed_editor_size(accepted, None), None);
        for invalid in [(0, 0), (0, 600), (800, 0)] {
            assert_eq!(changed_editor_size(accepted, Some(invalid)), None);
        }
    }

    #[test]
    fn close_survives_contention_and_is_consumed_once() {
        let plugin = std::sync::Mutex::new(());
        let guard = plugin.lock().unwrap();
        let mut pending = EditorHostEvents::default();
        pending.merge(EditorHostEvents {
            close_requested: true,
            resized: None,
        });
        for _ in 0..20 {
            pending.merge(EditorHostEvents::default());
            assert!(plugin.try_lock().is_err());
            assert!(pending.close_requested);
        }
        drop(guard);
        let _guard = plugin.try_lock().unwrap();
        assert!(std::mem::take(&mut pending).close_requested);
        assert_eq!(pending, EditorHostEvents::default());
    }

    #[test]
    fn latest_resize_wins_until_close_supersedes_it() {
        let mut pending = EditorHostEvents::default();
        for size in [(400, 200), (800, 600)] {
            pending.merge(EditorHostEvents {
                close_requested: false,
                resized: Some(size),
            });
        }
        assert_eq!(pending.resized, Some((800, 600)));
        pending.merge(EditorHostEvents {
            close_requested: true,
            resized: None,
        });
        pending.merge(EditorHostEvents {
            close_requested: false,
            resized: Some((900, 700)),
        });
        assert!(pending.close_requested);
        assert_eq!(pending.resized, None);
    }
}
