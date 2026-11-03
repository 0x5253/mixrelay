//! Main-thread `WebKit` setup before the first page or rendering backend exists.
use super::{DeferredWindows, Runtime, defer_windows};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_foundation::NSProcessInfo;
use objc2_web_kit::WKWebViewConfiguration;
use std::ffi::{c_char, c_void};
use tauri::Manager;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("WebKit configuration requires the application main thread")]
    MainThread,
    #[error("WebKit DOM rendering feature could not be configured: status {0}")]
    Feature(i32),
    #[error(transparent)]
    Window(#[from] tauri::Error),
}

unsafe extern "C" {
    fn mr_webkit_set_bool_feature(
        preferences: *const c_void,
        key: *const c_char,
        enabled: bool,
    ) -> i32;
}

pub(crate) fn prepare(config: &mut tauri::Config) -> DeferredWindows {
    let process = NSProcessInfo::processInfo();
    let version = process.operatingSystemVersion();
    defer_windows(
        config,
        Runtime {
            major: version.majorVersion,
            minor: version.minorVersion,
            processors: process.processorCount(),
        },
    )
}

fn configuration(main: MainThreadMarker) -> Result<Retained<WKWebViewConfiguration>, Error> {
    // SAFETY: main proves AppKit thread ownership; this fresh configuration is
    // retained here and has not yet been attached to a webview.
    let configuration = unsafe { WKWebViewConfiguration::new(main) };
    // SAFETY: configuration is a live, main-thread-owned WebKit instance.
    let preferences = unsafe { configuration.preferences() };
    // SAFETY: preferences remains retained for this call, the key is a static
    // terminated C string, and the bridge catches Objective-C exceptions.
    let status = unsafe {
        mr_webkit_set_bool_feature(
            Retained::as_ptr(&preferences).cast(),
            c"UseGPUProcessForDOMRenderingEnabled".as_ptr(),
            true,
        )
    };
    if status != 0 {
        return Err(Error::Feature(status));
    }
    Ok(configuration)
}

pub(crate) fn create_deferred(app: &tauri::App) -> Result<(), Error> {
    let deferred = app.state::<DeferredWindows>();
    if deferred.0.is_empty() {
        return Ok(());
    }
    let main = MainThreadMarker::new().ok_or(Error::MainThread)?;
    for window in &deferred.0 {
        tauri::WebviewWindowBuilder::from_config(app, window)?
            .with_webview_configuration(configuration(main)?)
            .build()?;
    }
    Ok(())
}
