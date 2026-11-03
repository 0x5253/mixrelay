//! Routes native Quit through each product's acknowledged shutdown path.
use std::{ffi::c_char, sync::Mutex};

use objc2::{
    MainThreadMarker,
    runtime::{AnyClass, AnyObject, Bool, Sel},
    sel,
};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};

static APPLICATION: Mutex<Option<tauri::AppHandle>> = Mutex::new(None);

type QuitMethod = extern "C-unwind" fn(&AnyObject, Sel, &AnyObject) -> NSApplicationTerminateReply;

#[link(name = "objc")]
unsafe extern "C" {
    // This fixed callback type avoids casting between incompatible function
    // pointers. The runtime stores the IMP together with the matching encoding.
    #[link_name = "class_addMethod"]
    fn add_method(
        class: *const AnyClass,
        selector: Sel,
        method: QuitMethod,
        types: *const c_char,
    ) -> Bool;
}

/// Keeps the request target alive until desktop state is released.
pub(crate) struct NativeTermination;

impl Drop for NativeTermination {
    fn drop(&mut self) {
        release();
    }
}

/// Breaks the retained app-handle link on the final run event, after shutdown.
pub(crate) fn release() {
    let app = APPLICATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    drop(app);
}

/// Adds the missing native termination decision without replacing Tauri's delegate.
/// Returns an error for missing/main-thread state or an existing decision method.
pub(crate) fn install(app: &tauri::AppHandle) -> Result<NativeTermination, String> {
    let main = MainThreadMarker::new().ok_or("termination hook requires the main thread")?;
    let application = NSApplication::sharedApplication(main);
    let delegate = application
        .delegate()
        .ok_or("application delegate is unavailable")?;
    let mut target = APPLICATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if target.is_some() {
        return Err("native termination hook is already installed".into());
    }
    let object = AsRef::<AnyObject>::as_ref(&*delegate);
    install_method(object.class())?;
    *target = Some(app.clone());
    drop(target);
    // Refresh AppKit's optional-delegate-method cache after the runtime addition.
    // The exact same retained delegate is preserved; no ivars or ownership change.
    application.setDelegate(Some(&delegate));
    Ok(NativeTermination)
}

fn install_method(class: &AnyClass) -> Result<(), String> {
    let selector = sel!(applicationShouldTerminate:);
    if class.instance_method(selector).is_some() {
        return Err("application delegate already handles native termination".into());
    }
    // SAFETY: setup runs on the main thread before user events. This adds only a
    // missing method, never replaces one (including inherited methods). The static
    // function receives two live Objective-C objects and returns NSUInteger; both
    // supported macOS ABIs encode that as Q. Runtime class/method storage is immortal.
    if !unsafe { add_method(class, selector, should_terminate, c"Q@:@".as_ptr()) }.as_bool() {
        return Err("could not install native termination decision".into());
    }
    Ok(())
}

extern "C-unwind" fn should_terminate(
    _delegate: &AnyObject,
    _selector: Sel,
    _application: &AnyObject,
) -> NSApplicationTerminateReply {
    let app = APPLICATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(app) = app {
        // AppKit termination bypasses Tauri's ExitRequested event. Re-enter
        // that managed path so Driver/device or App/engine shutdown completes
        // before exit. Window dismissal still follows the separate hide path.
        app.exit(0);
    }
    // AppKit must not exit immediately: the managed shutdown owns the decision.
    // Explicit tray/menu Quit already uses that same path.
    NSApplicationTerminateReply::TerminateCancel
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::{
        ClassType, Encode, msg_send,
        runtime::{ClassBuilder, NSObject},
    };

    #[test]
    fn native_method_cancels_immediate_exit_and_refuses_replacement() {
        assert_eq!(NSApplicationTerminateReply::ENCODING.to_string(), "Q");
        let class = ClassBuilder::new(c"MixRelayTerminationTest", NSObject::class())
            .unwrap()
            .register();
        install_method(class).unwrap();
        assert!(install_method(class).is_err());
        // SAFETY: the class inherits NSObject allocation/initialization and the
        // installed method's signature is verified by objc2's message encoding.
        let object: objc2::rc::Retained<NSObject> = unsafe { msg_send![class, new] };
        let result: NSApplicationTerminateReply =
            unsafe { msg_send![&object, applicationShouldTerminate: &*object] };
        assert_eq!(result, NSApplicationTerminateReply::TerminateCancel);
        let child = ClassBuilder::new(c"MixRelayTerminationChildTest", class)
            .unwrap()
            .register();
        assert!(install_method(child).is_err());
    }
}
