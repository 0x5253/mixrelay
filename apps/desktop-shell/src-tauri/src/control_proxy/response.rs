//! The engine's answer as the editor receives it, including the answers the desktop gives
//! itself when a request cannot be sent or completed.
use mixrelay_desktop_core::engine_control::Delivery;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlProxyResponse {
    /// The engine incarnation that answered, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) engine: Option<String>,
    pub(crate) status: u16,
    pub(crate) body: Value,
    /// Only macOS capture ownership distinguishes unsent from uncertain requests.
    #[cfg(target_os = "macos")]
    #[serde(skip)]
    pub(super) delivery: Delivery,
}

impl ControlProxyResponse {
    /// An answer the engine sent.
    pub(crate) fn answered(engine: Option<String>, status: u16, body: Value) -> Self {
        Self::delivered(engine, status, body, Delivery::ServerResponse)
    }

    pub(super) fn delivered(
        engine: Option<String>,
        status: u16,
        body: Value,
        delivery: Delivery,
    ) -> Self {
        #[cfg(target_os = "linux")]
        let _ = delivery;
        Self {
            engine,
            status,
            body,
            #[cfg(target_os = "macos")]
            delivery,
        }
    }

    /// Refused by the desktop; nothing was sent, so nothing was applied.
    pub(crate) fn rejected(code: &str) -> Self {
        Self::delivered(
            None,
            409,
            json!({"code": code, "parameters": {"nodeControlOutcome": "not_applied"}}),
            Delivery::NotSent,
        )
    }

    /// A command that failed before its outcome was known; it may have reached the engine.
    pub(super) fn unavailable(reason: &str) -> Self {
        Self::delivered(
            None,
            503,
            json!({"code": "control_unavailable", "parameters": {"reason": reason}}),
            Delivery::Uncertain,
        )
    }

    /// Application captures could not be prepared, so the submission was not sent.
    #[cfg(target_os = "macos")]
    pub(super) fn capture_unavailable(reason: &str) -> Self {
        Self::delivered(
            None,
            503,
            json!({"code": "audio_source_unavailable", "parameters": {"reason": reason}}),
            Delivery::NotSent,
        )
    }

    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_engine_survives_runtime_and_webview_serialization() {
        let unidentified = json!({"status":200,"body":{}});
        let mut response: ControlProxyResponse =
            serde_json::from_value(unidentified.clone()).unwrap();
        assert!(response.engine.is_none());
        assert_eq!(serde_json::to_value(&response).unwrap(), unidentified);
        response.engine = Some("engine".into());
        let wire = serde_json::to_value(response).unwrap();
        assert_eq!(wire["engine"], "engine");
        assert_eq!(
            serde_json::from_value::<ControlProxyResponse>(wire)
                .unwrap()
                .engine
                .as_deref(),
            Some("engine")
        );
    }

    #[test]
    fn a_local_refusal_tells_the_editor_nothing_was_applied() {
        let refused = ControlProxyResponse::rejected("graph_exchange_busy");
        assert_eq!(refused.status, 409);
        assert!(!refused.is_success());
        assert_eq!(
            serde_json::to_value(&refused).unwrap(),
            json!({"status":409,"body":{"code":"graph_exchange_busy",
                "parameters":{"nodeControlOutcome":"not_applied"}}})
        );
        #[cfg(target_os = "macos")]
        assert_eq!(refused.delivery, Delivery::NotSent);
    }
}
