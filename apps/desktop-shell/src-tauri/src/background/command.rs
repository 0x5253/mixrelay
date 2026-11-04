//! The closed set of native commands the background controller serves, by wire name.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeCommand {
    /// Every MIDI command; the MIDI service selects by name.
    Midi,
    Hello,
    SetNativeMenuLocale,
    EngineRequest,
    WorkspacePath,
    WorkspaceSubmit,
    GetGraphLayout,
    SetGraphLayout,
    GetStatus,
    ReconfigureSession,
    SetEnginePower,
    GetGraph,
    SetVirtualEndpointSampleRate,
    GetOperation,
    GetGraphStatus,
    GetGraphCatalog,
    MutateGraphCatalog,
    GetMeters,
    GetProcessors,
    RetryProcessor,
    GetProcessorEditor,
    RescanProcessors,
    SetProcessorParameter,
    SetProcessorEditor,
    GetProcessorEditorFrame,
    SendProcessorEditorInput,
    ApplyGraph,
    GetNodeControls,
    UpdateNodeControl,
    GetRecorderStatus,
    SetRecorder,
    RecordingSettings,
    SaveRecordingSettings,
    DriverStatus,
    DriverMutate,
    ListCapturableApplications,
    ListAudioDevices,
    StartCaptureProbe,
    CaptureProbeStats,
    StopCaptureProbe,
}

const NAMES: &[(&str, NativeCommand)] = &[
    ("midi_map", NativeCommand::Midi),
    ("midi_status", NativeCommand::Midi),
    ("midi_learn", NativeCommand::Midi),
    ("midi_cancel", NativeCommand::Midi),
    ("midi_save", NativeCommand::Midi),
    ("midi_remove", NativeCommand::Midi),
    ("hello", NativeCommand::Hello),
    ("set_native_menu_locale", NativeCommand::SetNativeMenuLocale),
    ("engine_request", NativeCommand::EngineRequest),
    ("workspace_path", NativeCommand::WorkspacePath),
    ("workspace_submit", NativeCommand::WorkspaceSubmit),
    ("control_get_graph_layout", NativeCommand::GetGraphLayout),
    ("control_set_graph_layout", NativeCommand::SetGraphLayout),
    ("control_get_status", NativeCommand::GetStatus),
    (
        "control_reconfigure_session",
        NativeCommand::ReconfigureSession,
    ),
    ("control_set_engine_power", NativeCommand::SetEnginePower),
    ("control_get_graph", NativeCommand::GetGraph),
    (
        "control_set_virtual_endpoint_sample_rate",
        NativeCommand::SetVirtualEndpointSampleRate,
    ),
    ("control_get_operation", NativeCommand::GetOperation),
    ("control_get_graph_status", NativeCommand::GetGraphStatus),
    ("control_get_graph_catalog", NativeCommand::GetGraphCatalog),
    (
        "control_mutate_graph_catalog",
        NativeCommand::MutateGraphCatalog,
    ),
    ("control_get_meters", NativeCommand::GetMeters),
    ("control_get_processors", NativeCommand::GetProcessors),
    ("control_retry_processor", NativeCommand::RetryProcessor),
    (
        "control_get_processor_editor",
        NativeCommand::GetProcessorEditor,
    ),
    ("control_rescan_processors", NativeCommand::RescanProcessors),
    (
        "control_set_processor_parameter",
        NativeCommand::SetProcessorParameter,
    ),
    (
        "control_set_processor_editor",
        NativeCommand::SetProcessorEditor,
    ),
    (
        "control_get_processor_editor_frame",
        NativeCommand::GetProcessorEditorFrame,
    ),
    (
        "control_send_processor_editor_input",
        NativeCommand::SendProcessorEditorInput,
    ),
    ("control_apply_graph", NativeCommand::ApplyGraph),
    ("control_get_node_controls", NativeCommand::GetNodeControls),
    (
        "control_update_node_control",
        NativeCommand::UpdateNodeControl,
    ),
    (
        "control_get_recorder_status",
        NativeCommand::GetRecorderStatus,
    ),
    ("control_set_recorder", NativeCommand::SetRecorder),
    ("recording_settings", NativeCommand::RecordingSettings),
    (
        "save_recording_settings",
        NativeCommand::SaveRecordingSettings,
    ),
    ("driver_status", NativeCommand::DriverStatus),
    ("driver_mutate", NativeCommand::DriverMutate),
    (
        "list_capturable_applications",
        NativeCommand::ListCapturableApplications,
    ),
    ("list_audio_devices", NativeCommand::ListAudioDevices),
    ("start_capture_probe", NativeCommand::StartCaptureProbe),
    ("capture_probe_stats", NativeCommand::CaptureProbeStats),
    ("stop_capture_probe", NativeCommand::StopCaptureProbe),
];

impl NativeCommand {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|&(_, command)| command)
    }

    /// Whether the editor sends a webview invocation of this command to the controller.
    /// The handshake and the editor proxy's engine and workspace requests use the
    /// connection directly, and each process sets the locale of its own menu.
    pub(crate) fn is_forwarded_from_editor(self) -> bool {
        !matches!(
            self,
            Self::Hello
                | Self::SetNativeMenuLocale
                | Self::EngineRequest
                | Self::WorkspacePath
                | Self::WorkspaceSubmit
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wire_name_selects_one_command() {
        let mut names: Vec<_> = NAMES.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), NAMES.len());
        for (name, command) in NAMES {
            assert_eq!(NativeCommand::parse(name), Some(*command));
        }
    }

    #[test]
    fn the_editor_forwards_audio_commands_and_keeps_dialogs_and_controller_internals() {
        let forwarded =
            |name| NativeCommand::parse(name).is_some_and(NativeCommand::is_forwarded_from_editor);
        for name in [
            "midi_learn",
            "control_apply_graph",
            "control_get_graph_catalog",
            "driver_mutate",
            "list_audio_devices",
            "start_capture_probe",
        ] {
            assert!(forwarded(name), "{name}");
        }
        for name in [
            "hello",
            "engine_request",
            "workspace_submit",
            "set_native_menu_locale",
            "select_processor_directory",
            "list_device_app_activity",
            "updates_check",
        ] {
            assert!(!forwarded(name), "{name}");
        }
    }
}
