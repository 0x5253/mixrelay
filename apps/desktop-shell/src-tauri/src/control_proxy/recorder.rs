use mixrelay_desktop_core::engine_control::routes;
use reqwest::Method;
use serde_json::Value;

use super::{ControlProxyClient, ControlProxyResponse};

#[tauri::command]
pub(crate) async fn control_get_recorder_status(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
    node_id: String,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::GET, &routes::recorder(&graph_id, &node_id), None)
        .await
}

#[tauri::command]
pub(crate) async fn control_set_recorder(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
    node_id: String,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    client.submit_recorder(&graph_id, &node_id, request).await
}

impl ControlProxyClient {
    /// UI and local controllers submit the same action using desktop-owned next-take settings.
    pub(super) async fn submit_recorder(
        &self,
        graph_id: &str,
        node_id: &str,
        mut request: Value,
    ) -> Result<ControlProxyResponse, String> {
        let _commits = self.recordings.lock_commits().await;
        let status = self.recording_status(graph_id, node_id).await?;
        request["request"] = mixrelay_desktop_core::recording::prepare_transport(
            &request["request"],
            &status,
            &self.recordings.preferences(graph_id, node_id)?,
            self.recordings.take_identities(),
        )
        .map_err(|error| error.to_string())?;
        if request["expectedRecording"].is_null() {
            request["expectedRecording"] =
                serde_json::json!({"state":status["state"],"path":status["path"]});
        }
        self.request(
            Method::PUT,
            &routes::recorder(graph_id, node_id),
            Some(request),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_desktop_core::recording::{
        RecordingEncoding, RecordingError, RecordingSettings, TakeIdentities, TakeIdentity,
    };

    struct FixedTake;
    impl TakeIdentities for FixedTake {
        fn next(&self) -> Result<TakeIdentity, RecordingError> {
            Ok(TakeIdentity {
                nanos: 1,
                owner: 2,
                sequence: 3,
            })
        }
    }

    fn prepare_transport(
        action: &Value,
        status: &Value,
        settings: &RecordingSettings,
    ) -> Result<Value, RecordingError> {
        mixrelay_desktop_core::recording::prepare_transport(action, status, settings, &FixedTake)
    }

    #[test]
    fn transport_uses_shared_format_only_for_new_takes() {
        let directory = tempfile::tempdir().unwrap();
        let settings = RecordingSettings {
            folder: directory.path().to_string_lossy().into_owned(),
            sample_rate: Some(96_000),
            encoding: RecordingEncoding::Pcm24,
        };
        let start = prepare_transport(
            &serde_json::json!({"action":"record"}),
            &serde_json::json!({"state":"stopped"}),
            &settings,
        )
        .unwrap();
        assert_eq!(start["sampleRate"], 96_000);
        assert_eq!(start["encoding"], "pcm24");
        for action in ["record", "pause", "stop"] {
            let request = serde_json::json!({"action":action});
            assert_eq!(
                prepare_transport(&request, &serde_json::json!({"state":"paused"}), &settings)
                    .unwrap(),
                request
            );
        }
        assert!(
            prepare_transport(
                &serde_json::json!({"action":"record", "path":"/unlinked.wav"}),
                &serde_json::json!({"state":"stopped"}),
                &settings
            )
            .is_err()
        );
    }
}
