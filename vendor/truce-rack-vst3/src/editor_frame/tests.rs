//! Native editor binding and run-loop fixtures.

use super::EditorBinding;
use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;
use vst3::Steinberg::{
    FIDString, IPlugFrame, IPlugFrameTrait, IPlugView, IPlugViewTrait, ViewRect, kInvalidArgument,
    kResultOk,
};
use vst3::{Class, ComWrapper};

struct FakeViewState {
    frame: *mut IPlugFrame,
    self_ptr: *mut IPlugView,
    set_frame_result: i32,
    attached_result: i32,
    attach_saw_frame: bool,
    removed_count: usize,
    set_frame_count: usize,
    on_size_count: usize,
    size: ViewRect,
}

struct FakeView(Mutex<FakeViewState>);

impl Class for FakeView {
    type Interfaces = (IPlugView,);
}

impl IPlugViewTrait for FakeView {
    unsafe fn isPlatformTypeSupported(&self, _type: FIDString) -> i32 {
        kResultOk
    }

    unsafe fn attached(&self, _parent: *mut c_void, _type: FIDString) -> i32 {
        let (frame, status) = {
            let mut state = self.0.lock().expect("fake view state");
            state.attach_saw_frame = !state.frame.is_null();
            (state.frame, state.attached_result)
        };
        if !frame.is_null() {
            let mut rect = ViewRect {
                left: 0,
                top: 0,
                right: 640,
                bottom: 480,
            };
            // SAFETY: the stored frame is borrowed from the binding and remains live through
            // the synchronous attached call; this exercises resizeView's onSize contract.
            if let Some(frame_ref) = unsafe { vst3::ComRef::<IPlugFrame>::from_raw(frame) } {
                let view = self.0.lock().expect("fake view state").self_ptr;
                unsafe { frame_ref.resizeView(view, &raw mut rect) };
            }
        }
        status
    }

    unsafe fn removed(&self) -> i32 {
        self.0.lock().expect("fake view state").removed_count += 1;
        kResultOk
    }

    unsafe fn onWheel(&self, _distance: f32) -> i32 {
        kResultOk
    }

    unsafe fn onKeyDown(&self, _key: u16, _key_code: i16, _modifiers: i16) -> i32 {
        kResultOk
    }

    unsafe fn onKeyUp(&self, _key: u16, _key_code: i16, _modifiers: i16) -> i32 {
        kResultOk
    }

    unsafe fn getSize(&self, size: *mut ViewRect) -> i32 {
        if size.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: VST3 requires this output pointer to be writable.
        unsafe { *size = self.0.lock().expect("fake view state").size };
        kResultOk
    }

    unsafe fn onSize(&self, new_size: *mut ViewRect) -> i32 {
        if new_size.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: VST3 requires the supplied rectangle to remain readable for this call.
        let rect = unsafe { *new_size };
        let mut state = self.0.lock().expect("fake view state");
        state.on_size_count += 1;
        state.size = rect;
        kResultOk
    }

    unsafe fn onFocus(&self, _state: u8) -> i32 {
        kResultOk
    }

    unsafe fn setFrame(&self, frame: *mut IPlugFrame) -> i32 {
        let mut state = self.0.lock().expect("fake view state");
        state.frame = frame;
        state.set_frame_count += 1;
        if frame.is_null() {
            kResultOk
        } else {
            state.set_frame_result
        }
    }

    unsafe fn canResize(&self) -> i32 {
        kResultOk
    }

    unsafe fn checkSizeConstraint(&self, _rect: *mut ViewRect) -> i32 {
        kResultOk
    }
}

fn fake_view(attached_result: i32, set_frame_result: i32) -> ComWrapper<FakeView> {
    let view = ComWrapper::new(FakeView(Mutex::new(FakeViewState {
        frame: ptr::null_mut(),
        self_ptr: ptr::null_mut(),
        set_frame_result,
        attached_result,
        attach_saw_frame: false,
        removed_count: 0,
        set_frame_count: 0,
        on_size_count: 0,
        size: ViewRect {
            left: 0,
            top: 0,
            right: 320,
            bottom: 240,
        },
    })));
    let interface = view.as_com_ref::<IPlugView>().expect("fake view interface");
    view.0.lock().expect("fake view state").self_ptr = interface.as_ptr();
    view
}

#[test]
fn frame_is_set_before_attach_and_unset_after_close() {
    let view_wrapper = fake_view(kResultOk, kResultOk);
    let view = view_wrapper
        .to_com_ptr::<IPlugView>()
        .expect("fake view interface");
    let binding = EditorBinding::attach(view, ptr::null_mut(), c"test-host-parent".as_ptr())
        .expect("editor attaches");
    {
        let state = view_wrapper.0.lock().expect("fake view state");
        assert!(state.attach_saw_frame);
        assert_eq!(state.on_size_count, 1);
        assert_eq!(state.size.right, 640);
        assert_eq!(state.size.bottom, 480);
        assert!(!state.frame.is_null());
    }
    drop(binding);
    let state = view_wrapper.0.lock().expect("fake view state");
    assert_eq!(state.removed_count, 1);
    assert_eq!(state.set_frame_count, 2);
    assert!(state.frame.is_null());
}

#[test]
fn rejected_attach_rolls_back_frame_and_partial_view() {
    let view_wrapper = fake_view(kInvalidArgument, kResultOk);
    let view = view_wrapper
        .to_com_ptr::<IPlugView>()
        .expect("fake view interface");
    let result = EditorBinding::attach(view, ptr::null_mut(), c"test-host-parent".as_ptr());
    assert!(result.is_err());
    let state = view_wrapper.0.lock().expect("fake view state");
    assert!(state.attach_saw_frame);
    assert_eq!(state.removed_count, 1);
    assert_eq!(state.set_frame_count, 2);
    assert!(state.frame.is_null());
}

#[test]
fn rejected_frame_binding_is_cleared_without_removing_unattached_view() {
    let view_wrapper = fake_view(kResultOk, kInvalidArgument);
    let view = view_wrapper
        .to_com_ptr::<IPlugView>()
        .expect("fake view interface");
    let result = EditorBinding::attach(view, ptr::null_mut(), c"test-host-parent".as_ptr());
    assert!(result.is_err());
    let state = view_wrapper.0.lock().expect("fake view state");
    assert_eq!(state.removed_count, 0);
    assert_eq!(state.set_frame_count, 2);
    assert!(state.frame.is_null());
}
