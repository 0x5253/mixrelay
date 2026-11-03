use super::{EditorHostEvents, EditorHostFailure};
use crate::shared_transport::WorkerControl;
use crate::{
    ProcessorStateError,
    editor_surface::{EDITOR_FRAME, PointerEvent},
    rack_formats::{EditorSnapshot, RackWorkerPlugin},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use block2::RcBlock;
use dispatch2::MainThreadBound;
use mixrelay_control_protocol::{EditorPointerKind, ProcessorEditorFrame};
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapImageFileType,
    NSEvent, NSEventMask, NSEventModifierFlags, NSEventTrackingRunLoopMode, NSEventType, NSView,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSDate, NSDefaultRunLoopMode, NSDictionary, NSPoint, NSRect, NSRunLoop, NSSize, NSString,
    NSTimer,
};
use std::{
    cell::Cell,
    ptr::NonNull,
    sync::{Arc, atomic::Ordering},
};
use truce_rack_core::WindowHandle;

pub(crate) struct NativeEditorHost {
    shared: Arc<WorkerControl>,
    /// Proof that this host was created on, and stays bound to, the main thread.
    main: MainThreadMarker,
    app: Retained<NSApplication>,
    surface: Option<Surface>,
    pub(super) pending_events: EditorHostEvents,
}

struct Surface {
    timer: Retained<NSTimer>,
    state: Arc<MainThreadBound<SurfaceState>>,
}

struct SurfaceState {
    window: Retained<NSWindow>,
    view: Retained<NSView>,
    shared: Arc<WorkerControl>,
    session: u64,
    size: Cell<(u32, u32)>,
    resizable: Cell<bool>,
    pressed: Cell<bool>,
    point: Cell<NSPoint>,
    capturing: Cell<bool>,
}

impl NativeEditorHost {
    pub(crate) fn new(shared: Arc<WorkerControl>) -> Result<Self, EditorHostFailure> {
        let main = MainThreadMarker::new().ok_or(EditorHostFailure::DisplayUnavailable)?;
        let app = NSApplication::sharedApplication(main);
        if !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
            return Err(EditorHostFailure::DisplayUnavailable);
        }
        app.finishLaunching();
        // AppKit initializes its coordinate conversion on the first event read.
        for _ in 0..32 {
            let Some(event) = next_event(&app, NSEventMask::Any) else {
                break;
            };
            app.sendEvent(&event);
        }
        Ok(Self {
            shared,
            main,
            app,
            surface: None,
            pending_events: EditorHostEvents::default(),
        })
    }

    pub(crate) fn open(
        &mut self,
        plugin: &mut RackWorkerPlugin,
        title: &str,
    ) -> Result<EditorSnapshot, EditorHostFailure> {
        let main = MainThreadMarker::new().ok_or(EditorHostFailure::DisplayUnavailable)?;
        if let Some(surface) = &self.surface {
            if surface.state.get(main).session
                == (self.shared)
                    .header()
                    .editor_sequence
                    .load(Ordering::Acquire)
            {
                return Ok(plugin.tick_editor());
            }
            self.close(plugin);
        }
        let window = parent_window(main);
        let view = window.contentView().ok_or(EditorHostFailure::HostWindow)?;
        let result = (|| {
            let snapshot = plugin
                .open_editor(
                    WindowHandle::NSView(Retained::as_ptr(&view).cast_mut().cast()),
                    1.0,
                )
                .map_err(|error| {
                    tracing::warn!(%error, "native editor open failed");
                    EditorHostFailure::PluginRejected
                })?;
            let (width, height) = snapshot.size.ok_or_else(|| {
                tracing::warn!("native editor did not report a size after opening");
                EditorHostFailure::PluginRejected
            })?;
            let scale = window.backingScaleFactor();
            if !valid_size((width, height), scale) {
                tracing::warn!(width, height, scale, "native editor surface size rejected");
                return Err(EditorHostFailure::PluginRejected);
            }
            window.setStyleMask(window_style(snapshot.resizable));
            window.setContentSize(NSSize::new(f64::from(width), f64::from(height)));
            window.setTitle(&NSString::from_str(title));
            window.center();
            self.app.activate();
            window.makeKeyAndOrderFront(None);
            if !window.isVisible() {
                return Err(EditorHostFailure::HostWindow);
            }
            let state = Arc::new(MainThreadBound::new(
                SurfaceState {
                    window: window.clone(),
                    view: view.clone(),
                    shared: Arc::clone(&self.shared),
                    session: (self.shared)
                        .header()
                        .editor_sequence
                        .load(Ordering::Acquire),
                    size: Cell::new((width, height)),
                    resizable: Cell::new(snapshot.resizable),
                    pressed: Cell::new(false),
                    point: Cell::new(NSPoint::new(0.0, 0.0)),
                    capturing: Cell::new(false),
                },
                main,
            ));
            self.surface = Some(Surface::new(state, main));
            Ok(snapshot)
        })();
        if result.is_err() {
            plugin.close_editor();
            window.close();
        }
        result
    }

    pub(crate) fn close(&mut self, plugin: &mut RackWorkerPlugin) {
        // Called only after event dispatch/tracking has unwound on this main thread.
        if let Some(surface) = &self.surface {
            surface.timer.invalidate();
        }
        plugin.close_editor();
        self.dismiss_surface();
        self.pending_events = EditorHostEvents::default();
        reject_pending_frame(&self.shared);
    }

    pub(super) fn collect_events(&mut self) -> EditorHostEvents {
        let Some(main) = MainThreadMarker::new() else {
            return EditorHostEvents::default();
        };
        service_idle_run_loop(self.surface.is_some(), || {
            // A failed/timed-out AUv3 open leaves no surface, but its callback may still
            // enqueue a Copy-rule controller release on dispatch's main queue. Service
            // one bounded main-run-loop turn on this off-RT broker thread.
            let mode = unsafe { NSDefaultRunLoopMode };
            let deadline = NSDate::dateWithTimeIntervalSinceNow(0.001);
            let _ = NSRunLoop::mainRunLoop().runMode_beforeDate(mode, &deadline);
        });
        let Some(surface) = &self.surface else {
            return EditorHostEvents::default();
        };
        let state = surface.state.get(main);
        state.tick(main);
        for _ in 0..64 {
            let Some(event) = next_event(&self.app, NSEventMask::Any) else {
                break;
            };
            self.app.sendEvent(&event);
        }
        if !state.window.isVisible() {
            return EditorHostEvents {
                close_requested: true,
                resized: None,
            };
        }
        let Some(size) = state.window_size() else {
            return EditorHostEvents {
                close_requested: true,
                resized: None,
            };
        };
        EditorHostEvents {
            close_requested: false,
            resized: (size != state.size.get()).then_some(size),
        }
    }

    pub(crate) fn refresh(&mut self, snapshot: EditorSnapshot) {
        if !snapshot.open {
            self.dismiss_surface();
            self.pending_events = EditorHostEvents::default();
            reject_pending_frame(&self.shared);
            return;
        }
        if let (Some(surface), Some(size), Some(main)) =
            (&self.surface, snapshot.size, MainThreadMarker::new())
        {
            let state = surface.state.get(main);
            if valid_size(size, state.window.backingScaleFactor()) {
                if state.window_size() != Some(size) {
                    state
                        .window
                        .setContentSize(NSSize::new(f64::from(size.0), f64::from(size.1)));
                }
                state.size.set(size);
                if state.resizable.replace(snapshot.resizable) != snapshot.resizable {
                    state.window.setStyleMask(window_style(snapshot.resizable));
                }
            } else {
                self.pending_events.close_requested = true;
            }
        }
    }

    fn dismiss_surface(&mut self) {
        if let Some(surface) = self.surface.take() {
            surface.timer.invalidate();
            let window = &surface.state.get(self.main).window;
            if window.isVisible() {
                window.close();
            }
        }
    }
}

impl Drop for NativeEditorHost {
    fn drop(&mut self) {
        self.dismiss_surface();
    }
}

impl Surface {
    fn new(state: Arc<MainThreadBound<SurfaceState>>, main: MainThreadMarker) -> Self {
        let captured = Arc::clone(&state);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(main) = MainThreadMarker::new() {
                captured.get(main).tick(main);
            }
        });
        // SAFETY: captured state is Send through MainThreadBound; access and destruction of
        // Cocoa objects remain on main. This timer is scheduled only on the main run loop.
        let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(0.008, true, &block) };
        let run_loop = NSRunLoop::mainRunLoop();
        // SAFETY: main-thread scheduling; immutable process-lifetime Foundation mode names.
        unsafe {
            run_loop.addTimer_forMode(&timer, NSDefaultRunLoopMode);
            run_loop.addTimer_forMode(&timer, NSEventTrackingRunLoopMode);
        }
        let _ = state.get(main);
        Self { timer, state }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // Surface is main-thread-owned by NativeEditorHost; callbacks cannot run
        // concurrently. Invalidating releases the block before the view is detached.
        self.timer.invalidate();
    }
}

impl SurfaceState {
    fn window_size(&self) -> Option<(u32, u32)> {
        let bounds = self.view.bounds();
        let size = (
            window_dimension(bounds.size.width)?,
            window_dimension(bounds.size.height)?,
        );
        valid_size(size, self.window.backingScaleFactor()).then_some(size)
    }

    fn tick(&self, main: MainThreadMarker) {
        let header = self.shared.header();
        let closing = header.editor_sequence.load(Ordering::Acquire) != self.session
            || header.shutdown.load(Ordering::Acquire) != 0;
        let cancelled = header.editor_input.take_cancel() == self.session;
        for _ in 0..64 {
            let Some(event) = header.editor_input.pop() else {
                break;
            };
            if !closing && !cancelled && event.session == self.session {
                self.post(event, main);
            }
        }
        if (closing || cancelled) && self.pressed.replace(false) {
            self.post_release(main);
        }
        let Some((request, EDITOR_FRAME)) = self.shared.pending_request() else {
            return;
        };
        if self.capturing.get() {
            return;
        }
        if closing {
            reject_pending_frame(&self.shared);
            return;
        }
        if self.capturing.replace(true) {
            return;
        }
        let result = self.frame();
        self.capturing.set(false);
        (self.shared).publish(request, result);
    }

    fn post(&self, event: PointerEvent, main: MainThreadMarker) {
        let kind = match event.kind {
            EditorPointerKind::Down => {
                self.pressed.set(true);
                NSEventType::LeftMouseDown
            }
            EditorPointerKind::Move if self.pressed.get() => NSEventType::LeftMouseDragged,
            EditorPointerKind::Move => return,
            EditorPointerKind::Up | EditorPointerKind::Cancel => {
                self.pressed.set(false);
                NSEventType::LeftMouseUp
            }
        };
        self.point.set(NSPoint::new(
            event.x,
            f64::from(self.size.get().1) - event.y,
        ));
        self.post_event(kind, event.modifiers, main);
    }

    fn post_release(&self, main: MainThreadMarker) {
        self.post_event(NSEventType::LeftMouseUp, 0, main);
    }

    fn post_event(&self, kind: NSEventType, modifiers: u8, main: MainThreadMarker) {
        let mut flags = NSEventModifierFlags::empty();
        for (bit, flag) in [
            (1, NSEventModifierFlags::Shift),
            (2, NSEventModifierFlags::Control),
            (4, NSEventModifierFlags::Option),
            (8, NSEventModifierFlags::Command),
        ] {
            if modifiers & bit != 0 {
                flags |= flag;
            }
        }
        if let Some(event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind, self.point.get(), flags, 0.0, self.window.windowNumber(), None, 0, 1, 1.0,
        ) { NSApplication::sharedApplication(main).postEvent_atStart(&event, false); }
    }

    fn frame(&self) -> Result<Vec<u8>, ProcessorStateError> {
        let bounds = self.view.bounds();
        let scale = self.window.backingScaleFactor();
        if !scale.is_finite()
            || scale <= 0.0
            || !bounds.size.width.is_finite()
            || !bounds.size.height.is_finite()
            || bounds.size.width <= 0.0
            || bounds.size.height <= 0.0
            || bounds.size.width * bounds.size.height * scale * scale > 4_000_000.0
        {
            return Err(ProcessorStateError::TooLarge);
        }
        let bitmap = self
            .view
            .bitmapImageRepForCachingDisplayInRect(bounds)
            .ok_or(ProcessorStateError::NativeFailure)?;
        self.view
            .cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);
        // SAFETY: empty properties dictionary selects PNG defaults; NSData owns its bytes.
        let png = unsafe {
            bitmap.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .ok_or(ProcessorStateError::NativeFailure)?;
        if png.len() > 3_000_000 {
            return Err(ProcessorStateError::TooLarge);
        }
        serde_json::to_vec(&ProcessorEditorFrame {
            instance_id: self.shared.header().instance_id.to_string(),
            session: self.session,
            width: self.size.get().0,
            height: self.size.get().1,
            png_base64: BASE64.encode(png.to_vec()),
        })
        .map_err(|_| ProcessorStateError::InvalidResponse)
    }
}

fn valid_size((width, height): (u32, u32), scale: f64) -> bool {
    width > 0
        && height > 0
        && scale.is_finite()
        && scale > 0.0
        && f64::from(width) * f64::from(height) * scale * scale <= 4_000_000.0
}

fn window_dimension(value: f64) -> Option<u32> {
    if !value.is_finite() || !(1.0..=4_000_000.0).contains(&value) {
        return None;
    }
    // Window geometry is control-plane data. Parse the rounded point count rather than
    // silently narrowing a CGFloat to an integer when the user drags a window edge.
    value.round().to_string().parse().ok()
}

fn window_style(resizable: bool) -> NSWindowStyleMask {
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
    if resizable {
        style | NSWindowStyleMask::Resizable
    } else {
        style
    }
}

fn reject_pending_frame(shared: &WorkerControl) {
    if let Some((request, EDITOR_FRAME)) = shared.pending_request() {
        (shared).publish(request, Err(ProcessorStateError::Unavailable));
    }
}

fn service_idle_run_loop(surface_open: bool, pump_once: impl FnOnce()) {
    if !surface_open {
        pump_once();
    }
}

fn next_event(app: &NSApplication, mask: NSEventMask) -> Option<Retained<NSEvent>> {
    // SAFETY: immutable process-lifetime Foundation constant, read on main thread.
    let mode = unsafe { NSDefaultRunLoopMode };
    app.nextEventMatchingMask_untilDate_inMode_dequeue(
        mask,
        Some(&NSDate::distantPast()),
        mode,
        true,
    )
}

fn parent_window(main: MainThreadMarker) -> Retained<NSWindow> {
    // SAFETY: AppKit window construction and release policy are confined to the main thread.
    // The host retains the window through plug-in detach, including after the user closes it.
    unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(main),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(500.0, 200.0)),
            window_style(false),
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window
    }
}

#[cfg(test)]
mod tests {
    use super::{service_idle_run_loop, window_dimension};

    #[test]
    fn idle_broker_services_main_queue_after_failed_open() {
        let mut turns = 0;
        service_idle_run_loop(false, || turns += 1);
        assert_eq!(turns, 1);
        service_idle_run_loop(true, || turns += 1);
        assert_eq!(turns, 1);
    }

    #[test]
    fn window_geometry_is_finite_positive_and_bounded() {
        assert_eq!(window_dimension(500.0), Some(500));
        assert_eq!(window_dimension(500.6), Some(501));
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY, 4_000_001.0] {
            assert_eq!(window_dimension(invalid), None);
        }
    }
}
