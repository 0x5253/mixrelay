use super::*;

#[test]
fn only_the_affected_low_processor_runtime_defers_automatic_creation() {
    for (major, minor, processors, affected) in [
        (27, 0, 3, true),
        (27, 0, 1, true),
        (27, 0, 4, false),
        (27, 1, 3, false),
        (26, 6, 3, false),
        (28, 0, 3, false),
        (27, 0, 0, false),
    ] {
        let runtime = Runtime {
            major,
            minor,
            processors,
        };
        let mut config = tauri::Config::default();
        let automatic = WindowConfig {
            label: "main".into(),
            width: 1180.0,
            height: 780.0,
            ..Default::default()
        };
        let manual = WindowConfig {
            label: "manual".into(),
            create: false,
            ..Default::default()
        };
        config.app.windows = vec![automatic.clone(), manual.clone()];
        let deferred = defer_windows(&mut config, runtime);
        assert_eq!(deferred.0.len(), usize::from(affected));
        assert_eq!(config.app.windows[0].create, !affected);
        assert_eq!(
            serde_json::to_value(&config.app.windows[1]).unwrap(),
            serde_json::to_value(&manual).unwrap()
        );
        if affected {
            assert_eq!(
                serde_json::to_value(&deferred.0[0]).unwrap(),
                serde_json::to_value(automatic).unwrap()
            );
        }
    }
}

#[test]
fn a_background_process_never_acquires_a_window() {
    let mut config = tauri::Config::default();
    let deferred = defer_windows(
        &mut config,
        Runtime {
            major: 27,
            minor: 0,
            processors: 3,
        },
    );
    assert!(deferred.0.is_empty());
    assert!(config.app.windows.is_empty());
}
