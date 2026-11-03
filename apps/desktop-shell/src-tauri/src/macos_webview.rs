//! `WebKit` configuration for macOS 27.0 guests with fewer than four processors.
use tauri::utils::config::WindowConfig;

#[derive(Debug)]
pub(crate) struct DeferredWindows(Vec<WindowConfig>);

#[derive(Clone, Copy, Debug)]
struct Runtime {
    major: isize,
    minor: isize,
    processors: usize,
}

impl Runtime {
    fn needs_remote_dom(self) -> bool {
        // WebKit 321119 removes this obsolete four-core gate. The 27.0
        // fallback requests IOSurfaces from a WebContent sandbox that denies them.
        self.major == 27 && self.minor == 0 && (1..4).contains(&self.processors)
    }
}

fn defer_windows(config: &mut tauri::Config, runtime: Runtime) -> DeferredWindows {
    let mut deferred = Vec::new();
    if runtime.needs_remote_dom() {
        for window in &mut config.app.windows {
            if window.create {
                deferred.push(window.clone());
                window.create = false;
            }
        }
    }
    DeferredWindows(deferred)
}

#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
pub(crate) use native::{create_deferred, prepare};

#[cfg(test)]
mod tests;
