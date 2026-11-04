//! Server rejections: the engine answered with an error, as opposed to a failure to reach it.
use serde_json::Value;

/// Outcome fields the engine reports on rejected mutations, by mutation kind.
const OUTCOME_FIELDS: [&str; 5] = [
    "operationOutcome",
    "catalogOutcome",
    "workspaceOutcome",
    "powerOutcome",
    "nodeControlOutcome",
];

/// What a rejection reveals. Paths, graph IDs and other parameters stay private.
#[derive(Debug, PartialEq, Eq)]
pub struct Rejection<'a> {
    pub code: &'a str,
    pub outcome: Option<&'static str>,
}

impl<'a> Rejection<'a> {
    /// The rejection carried by an error response with a coded body.
    #[must_use]
    pub fn from_response(status: u16, body: &'a Value) -> Option<Self> {
        if status < 400 {
            return None;
        }
        let code = body.get("code")?.as_str()?;
        let parameters = &body["parameters"];
        let outcome =
            OUTCOME_FIELDS
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

    #[test]
    fn unknown_outcome_values_are_not_reported() {
        let body = json!({"code":"busy", "parameters":{"powerOutcome":"/private/path"}});
        assert_eq!(
            Rejection::from_response(409, &body),
            Some(Rejection {
                code: "busy",
                outcome: None
            })
        );
    }
}
