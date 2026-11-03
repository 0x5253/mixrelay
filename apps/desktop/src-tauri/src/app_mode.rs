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
