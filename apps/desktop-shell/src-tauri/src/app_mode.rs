//! One executable runs as the graph editor or as the background runtime.
use std::ffi::OsStr;

/// The editor and the background runtime share one signed app bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppMode {
    Graph,
    Service,
}

impl AppMode {
    pub(crate) fn detect<'a>(mut arguments: impl Iterator<Item = &'a OsStr>) -> Self {
        if arguments.any(|argument| argument == "--driver-runtime") {
            Self::Service
        } else {
            Self::Graph
        }
    }

    /// The runtime has no editor webview or Dock window.
    pub(crate) fn configure(self, config: &mut tauri::Config) {
        if self == Self::Service {
            config.app.windows.clear();
        }
    }

    /// Each mode keeps its own log files.
    pub(crate) fn log_role(self) -> &'static str {
        match self {
            Self::Graph => "desktop",
            Self::Service => "driver-runtime",
        }
    }

    /// The Linux editor quits with its window; elsewhere closing only hides it. The runtime
    /// never quits from a window.
    pub(crate) fn quits_on_main_window_close(self) -> bool {
        cfg!(target_os = "linux") && self == Self::Graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_runtime_has_no_editor_window() {
        let mode = AppMode::detect([OsStr::new("--driver-runtime")].into_iter());
        assert_eq!(mode, AppMode::Service);
        let mut config = tauri::Config::default();
        config
            .app
            .windows
            .push(tauri::utils::config::WindowConfig::default());
        mode.configure(&mut config);
        assert!(config.app.windows.is_empty());
    }

    #[test]
    fn the_retired_status_menu_switch_opens_the_editor() {
        assert_eq!(AppMode::detect(std::iter::empty()), AppMode::Graph);
        assert_eq!(
            AppMode::detect([OsStr::new("--driver")].into_iter()),
            AppMode::Graph
        );
    }

    #[test]
    fn closing_a_window_never_quits_the_runtime() {
        assert!(!AppMode::Service.quits_on_main_window_close());
        assert_eq!(
            AppMode::Graph.quits_on_main_window_close(),
            cfg!(target_os = "linux")
        );
    }

    #[test]
    fn graph_mode_preserves_its_visible_resizable_window() {
        let mut config = tauri::Config::default();
        let window = tauri::utils::config::WindowConfig {
            width: 1180.0,
            height: 780.0,
            ..Default::default()
        };
        config.app.windows.push(window);
        AppMode::Graph.configure(&mut config);
        assert!(config.app.windows[0].visible);
        assert!(config.app.windows[0].resizable);
        assert_eq!(config.app.windows[0].width.to_bits(), 1180.0_f64.to_bits());
        assert_eq!(config.app.windows[0].height.to_bits(), 780.0_f64.to_bits());
    }
}
