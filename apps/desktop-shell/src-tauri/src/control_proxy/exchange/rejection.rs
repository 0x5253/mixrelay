//! Records server rejections separately from failures to reach the server.
use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
struct Rejection<'a> {
    code: &'a str,
    outcome: Option<&'static str>,
}

impl<'a> Rejection<'a> {
    fn from_response(status: u16, body: &'a Value) -> Option<Self> {
        if status < 400 {
            return None;
        }
        let code = body.get("code")?.as_str()?;
        let parameters = &body["parameters"];
        let outcome = [
            "operationOutcome",
            "catalogOutcome",
            "workspaceOutcome",
            "powerOutcome",
            "nodeControlOutcome",
        ]
        .into_iter()
        .find_map(|field| match parameters[field].as_str() {
            Some("not_admitted") => Some("not_admitted"),
            Some("not_applied") => Some("not_applied"),
            Some("uncertain") => Some("uncertain"),
            Some("pending") => Some("pending"),
            _ => None,
        });
        Some(Self { code, outcome })
    }
}

pub(super) fn observe(status: u16, body: &Value) {
    if let Some(rejection) = Rejection::from_response(status, body) {
        tracing::warn!(
            platform = ?crate::control_channel::failure_platform(),
            boundary = "runtime_engine",
            stage = "server_response",
            status,
            code = rejection.code,
            outcome = rejection.outcome,
            "engine rejected control request"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn completed_server_errors_preserve_their_code_and_admission_outcome() {
        let body = json!({"code":"control_unavailable", "parameters":{
            "catalogOutcome":"not_applied", "reason":"private path", "graphId":"private graph"
        }});
        assert_eq!(
            Rejection::from_response(503, &body),
            Some(Rejection {
                code: "control_unavailable",
                outcome: Some("not_applied")
            })
        );
        assert_eq!(Rejection::from_response(200, &body), None);
        assert_eq!(
            Rejection::from_response(503, &json!({"reason":"private path"})),
            None
        );
        assert_eq!(
            Rejection::from_response(404, &json!({"code":"node_not_found"})),
            Some(Rejection {
                code: "node_not_found",
                outcome: None
            })
        );
    }
}
