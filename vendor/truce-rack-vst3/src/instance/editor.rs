//! Own editor attachment and native view operations through close.

use super::Vst3Plugin;
use crate::editor_frame;
use truce_rack_core::error::Error;
use vst3::{
    ComPtr,
    Steinberg::{
        IPlugView, IPlugViewTrait, ViewRect,
        Vst::{IEditController, IEditControllerTrait, ViewType},
        kPlatformTypeHWND, kPlatformTypeNSView, kPlatformTypeX11EmbedWindowID, kResultOk,
    },
};

#[derive(Default)]
pub(super) struct EditorState {
    pub(super) binding: Option<editor_frame::EditorBinding>,
    pub(super) open: bool,
}
/// Call `IEditController::createView("editor")` and wrap the raw
/// pointer. Returns `None` when the plugin has no editor.
pub(super) unsafe fn create_editor_view(
    controller: &ComPtr<IEditController>,
) -> Option<ComPtr<IPlugView>> {
    let raw = unsafe { controller.createView(ViewType::kEditor) };
    if raw.is_null() {
        return None;
    }
    unsafe { ComPtr::<IPlugView>::from_raw(raw) }
}

fn platform_type_for_handle(
    handle: truce_rack_core::editor::WindowHandle,
) -> (vst3::Steinberg::FIDString, *mut std::ffi::c_void) {
    use truce_rack_core::editor::WindowHandle;
    match handle {
        WindowHandle::NSView(p) => (kPlatformTypeNSView, p),
        WindowHandle::HWND(p) => (kPlatformTypeHWND, p),
        WindowHandle::X11(id) => (kPlatformTypeX11EmbedWindowID, id as *mut std::ffi::c_void),
    }
}

impl truce_rack_core::editor::PluginEditor for Vst3Plugin {
    fn open(
        &mut self,
        parent: truce_rack_core::editor::WindowHandle,
        _scale: f64,
    ) -> truce_rack_core::error::Result<()> {
        if self.editor.open {
            return Ok(());
        }
        // View creation/attachment may expose mutable native UI state, including on partial
        // failure; do not retain a pre-editor latency snapshot.
        self.processing.latency_frames = None;
        self.native.host_services.invalidate_latency();
        let view = unsafe { create_editor_view(&self.native.controller) }
            .ok_or_else(|| Error::Other("IEditController::createView returned NULL".into()))?;
        let (type_str, parent_ptr) = platform_type_for_handle(parent);
        if unsafe { view.isPlatformTypeSupported(type_str) } != kResultOk {
            return Err(Error::Other(
                "IPlugView::isPlatformTypeSupported returned false".into(),
            ));
        }
        self.editor.binding = Some(
            editor_frame::EditorBinding::attach(view, parent_ptr, type_str)
                .map_err(|message| Error::Other(message.into()))?,
        );
        self.editor.open = true;
        Ok(())
    }

    fn close(&mut self) {
        self.editor.binding.take();
        self.editor.open = false;
        // The editor may have mutated latency through native UI state.
        self.processing.latency_frames = None;
        self.native.host_services.invalidate_latency();
    }

    fn is_open(&self) -> bool {
        self.editor.open
    }

    fn size(&self) -> Option<(u32, u32)> {
        let view = self.editor.binding.as_ref()?.view();
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { view.getSize(&raw mut rect) } != kResultOk {
            return None;
        }
        view_size(rect)
    }

    fn is_resizable(&self) -> bool {
        let Some(view) = self
            .editor
            .binding
            .as_ref()
            .map(editor_frame::EditorBinding::view)
        else {
            return false;
        };
        unsafe { view.canResize() == kResultOk }
    }

    fn set_size(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let view = self.editor.binding.as_ref()?.view();
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: i32::try_from(width).ok()?,
            bottom: i32::try_from(height).ok()?,
        };
        // Plugins may snap to a constraint via checkSizeConstraint.
        let _ = unsafe { view.checkSizeConstraint(&raw mut rect) };
        if unsafe { view.onSize(&raw mut rect) } != kResultOk {
            return None;
        }
        view_size(rect)
    }

    fn show(&mut self) {
        // VST3 attaches once and stays visible — no show/hide
        // separate from attached/removed.
    }

    fn hide(&mut self) {
        // Same as show — VST3 has no distinct hide.
    }

    fn on_idle(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(editor) = &self.editor.binding {
            editor.on_idle();
        }
    }
}

fn view_size(rect: ViewRect) -> Option<(u32, u32)> {
    Some((
        u32::try_from(rect.right.checked_sub(rect.left)?).ok()?,
        u32::try_from(rect.bottom.checked_sub(rect.top)?).ok()?,
    ))
}

#[cfg(test)]
mod tests;
