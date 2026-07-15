pub mod incoming;
pub mod outgoing;
pub mod shared;

use crate::version::ProtocolVersion;

pub use incoming::DebuggeeEvent;
pub use outgoing::DebuggerEvent;
pub use shared::*;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn parse_debuggee_message(value: serde_json::Value) -> Result<DebuggeeEvent, ParseError> {
    if value.get("type").and_then(|v| v.as_str()) == Some("event") {
        if let Some(inner) = value.get("event") {
            return parse_debuggee_message(inner.clone());
        }
    }
    let type_name = value.get("type").and_then(|v| v.as_str());
    if let Some(name) = type_name {
        if !KNOWN_INCOMING_TYPE_TAGS.contains(&name) {
            return Ok(DebuggeeEvent::Unknown {
                type_name: name.to_string(),
                data: value,
            });
        }
    }
    serde_json::from_value(value).map_err(Into::into)
}

pub fn encode_debugger_message(event: &DebuggerEvent) -> Result<serde_json::Value, ParseError> {
    serde_json::to_value(event).map_err(Into::into)
}

const KNOWN_INCOMING_TYPE_TAGS: &[&str] = &[
    "ProtocolEvent",
    "StoppedEvent",
    "ThreadEvent",
    "PrintEvent",
    "NotificationEvent",
    "StatEvent2",
    "ProfilerCapture",
    "debuggee-response",
    "SchemaEvent",
    "terminated",
];

pub fn encode_legacy_nested(
    _event: &DebuggerEvent,
    _version: ProtocolVersion,
) -> serde_json::Value {
    todo!("legacy nested encoding (protocol v5-v7) not yet implemented; only Cereal/flat form (v8+) is supported")
}

pub fn decode_legacy_nested(
    _value: serde_json::Value,
    _version: ProtocolVersion,
) -> Result<DebuggeeEvent, ParseError> {
    todo!("legacy nested decoding (protocol v5-v7) not yet implemented; only Cereal/flat form (v8+) is supported")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unwraps_event_envelope() {
        let wrapped = serde_json::json!({
            "type": "event",
            "event": {
                "type": "StoppedEvent",
                "reason": "breakpoint",
                "thread": 1
            }
        });
        let event = parse_debuggee_message(wrapped).unwrap();
        match event {
            DebuggeeEvent::Stopped { reason, thread } => {
                assert_eq!(reason, "breakpoint");
                assert_eq!(thread, 1);
            }
            other => panic!("expected Stopped, got {other:?}"),
        }
    }

    #[test]
    fn parse_passes_through_unwrapped_messages() {
        let direct = serde_json::json!({
            "type": "ProtocolEvent",
            "version": 9,
            "plugins": []
        });
        let event = parse_debuggee_message(direct).unwrap();
        assert!(matches!(event, DebuggeeEvent::Protocol { .. }));
    }

    #[test]
    fn parse_unwraps_double_nested_envelope() {
        let double_wrapped = serde_json::json!({
            "type": "event",
            "event": {
                "type": "event",
                "event": {
                    "type": "PrintEvent",
                    "message": "hi",
                    "logLevel": 0
                }
            }
        });
        let event = parse_debuggee_message(double_wrapped).unwrap();
        match event {
            DebuggeeEvent::Print { message, .. } => assert_eq!(message, "hi"),
            other => panic!("expected Print, got {other:?}"),
        }
    }

    #[test]
    fn parse_recognizes_terminated() {
        let value = serde_json::json!({"type": "terminated", "reason": "user exited"});
        let event = parse_debuggee_message(value).unwrap();
        match event {
            DebuggeeEvent::Terminated { reason } => {
                assert_eq!(reason.as_deref(), Some("user exited"));
            }
            other => panic!("expected Terminated, got {other:?}"),
        }
    }

    #[test]
    fn parse_falls_back_to_unknown_for_unrecognized_type() {
        let value = serde_json::json!({
            "type": "SomeNewEventType",
            "foo": "bar",
            "n": 42
        });
        let event = parse_debuggee_message(value.clone()).unwrap();
        match event {
            DebuggeeEvent::Unknown { type_name, data } => {
                assert_eq!(type_name, "SomeNewEventType");
                assert_eq!(data, value);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn parse_still_errors_for_known_type_with_bad_fields() {
        let value = serde_json::json!({"type": "StoppedEvent"});
        let result = parse_debuggee_message(value);
        assert!(result.is_err());
    }
}
