//! VST3 editor frame ownership and the Linux editor run loop.

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use vst3::Steinberg::{
    IPlugFrame, IPlugFrameTrait, IPlugView, IPlugViewTrait, ViewRect, kInvalidArgument, kResultOk,
    kResultTrue,
};
use vst3::{Class, ComPtr, ComRef, ComWrapper};

#[cfg(target_os = "linux")]
use vst3::Steinberg::Linux::{IEventHandler, IRunLoop, IRunLoopTrait, ITimerHandler};

#[cfg(target_os = "linux")]
mod run_loop;
#[cfg(target_os = "linux")]
use run_loop::RunLoopState;

/// A retained view and frame pair. Dropping it always removes the view, clears the frame,
/// disables callbacks, and releases the host's frame reference.
pub(super) struct EditorBinding {
    view: ComPtr<IPlugView>,
    frame: ComWrapper<EditorFrame>,
    attached_attempted: bool,
}

impl EditorBinding {
    /// Set a retained host frame before attaching `view` to `parent`.
    ///
    /// `parent` and `platform_type` are provided by the active native editor host. Returns an
    /// error when the plug-in rejects frame binding or attachment; partial state is rolled back.
    pub(super) fn attach(
        view: ComPtr<IPlugView>,
        parent: *mut std::ffi::c_void,
        platform_type: vst3::Steinberg::FIDString,
    ) -> Result<Self, &'static str> {
        let frame = ComWrapper::new(EditorFrame::new());
        frame.bind_view(view.as_ptr());
        let mut binding = Self {
            view,
            frame,
            attached_attempted: false,
        };
        let frame_ref = binding
            .frame
            .as_com_ref::<IPlugFrame>()
            .ok_or("host frame does not expose IPlugFrame")?;
        // SAFETY: the frame is retained by `binding` through detach and outlives this call.
        let frame_status = unsafe { binding.view.setFrame(frame_ref.as_ptr()) };
        if frame_status != kResultOk && frame_status != kResultTrue {
            return Err("IPlugView::setFrame failed");
        }
        binding.attached_attempted = true;
        // SAFETY: the caller supplies the live parent window and a supported platform type.
        if unsafe { binding.view.attached(parent, platform_type) } != kResultOk {
            return Err("IPlugView::attached returned non-OK");
        }
        Ok(binding)
    }

    /// Return the retained plug-in view for the existing editor size APIs.
    pub(super) fn view(&self) -> &ComPtr<IPlugView> {
        &self.view
    }

    #[cfg(target_os = "linux")]
    /// Pump registered Linux editor events on the host's existing UI/control thread.
    pub(super) fn on_idle(&self) {
        self.frame.service_run_loop();
    }
}

impl Drop for EditorBinding {
    fn drop(&mut self) {
        if self.attached_attempted {
            // SAFETY: the retained view is still alive and any partial attachment must be removed.
            unsafe { self.view.removed() };
        }
        self.frame.unbind_view();
        // SAFETY: null unsets the borrowed host frame before its owning wrapper is released.
        unsafe { self.view.setFrame(ptr::null_mut()) };
        #[cfg(target_os = "linux")]
        self.frame.close_run_loop();
    }
}

struct EditorFrame {
    // Weak identity only: EditorBinding owns the view until after this pointer is cleared.
    view: AtomicPtr<IPlugView>,
    #[cfg(target_os = "linux")]
    run_loop: RunLoopState,
}

impl EditorFrame {
    fn new() -> Self {
        Self {
            view: AtomicPtr::new(ptr::null_mut()),
            #[cfg(target_os = "linux")]
            run_loop: RunLoopState::new(),
        }
    }

    fn bind_view(&self, view: *mut IPlugView) {
        self.view.store(view, Ordering::Release);
    }

    fn unbind_view(&self) {
        self.view.store(ptr::null_mut(), Ordering::Release);
    }

    #[cfg(target_os = "linux")]
    fn service_run_loop(&self) {
        self.run_loop.service();
    }

    #[cfg(target_os = "linux")]
    fn close_run_loop(&self) {
        self.run_loop.close();
    }
}

#[cfg(target_os = "linux")]
impl Class for EditorFrame {
    type Interfaces = (IPlugFrame, IRunLoop);
}

#[cfg(not(target_os = "linux"))]
impl Class for EditorFrame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for EditorFrame {
    unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> i32 {
        if view.is_null() || new_size.is_null() || self.view.load(Ordering::Acquire) != view {
            return kInvalidArgument;
        }
        // SAFETY: the SDK keeps the callback's view and rectangle valid for this call. The stored
        // pointer check also rejects stale callbacks after detach without retaining the view.
        let Some(view_ref) = (unsafe { ComRef::from_raw(view) }) else {
            return kInvalidArgument;
        };
        // SAFETY: the SDK supplies a readable rectangle for this synchronous resize request.
        let requested = unsafe { *new_size };
        if requested
            .right
            .checked_sub(requested.left)
            .is_none_or(|width| width <= 0)
            || requested
                .bottom
                .checked_sub(requested.top)
                .is_none_or(|height| height <= 0)
        {
            return kInvalidArgument;
        }
        let mut current = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: the callback view remains live for the duration of resizeView.
        if unsafe { view_ref.getSize(&raw mut current) } == kResultOk
            && current.left == requested.left
            && current.top == requested.top
            && current.right == requested.right
            && current.bottom == requested.bottom
        {
            return kResultOk;
        }
        // The VST3 contract requires onSize in this same call stack after resizeView.
        unsafe { view_ref.onSize(new_size) }
    }
}

#[cfg(target_os = "linux")]
impl IRunLoopTrait for EditorFrame {
    unsafe fn registerEventHandler(
        &self,
        handler: *mut IEventHandler,
        fd: vst3::Steinberg::Linux::FileDescriptor,
    ) -> i32 {
        self.run_loop.register_event_handler(handler, fd)
    }

    unsafe fn unregisterEventHandler(&self, handler: *mut IEventHandler) -> i32 {
        self.run_loop.unregister_event_handler(handler)
    }

    unsafe fn registerTimer(
        &self,
        handler: *mut ITimerHandler,
        milliseconds: vst3::Steinberg::Linux::TimerInterval,
    ) -> i32 {
        self.run_loop.register_timer(handler, milliseconds)
    }

    unsafe fn unregisterTimer(&self, handler: *mut ITimerHandler) -> i32 {
        self.run_loop.unregister_timer(handler)
    }
}

#[cfg(test)]
mod tests;
