use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DebuggerEvent {
    #[serde(rename = "protocol")]
    Protocol {
        version: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_module_uuid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        passcode: Option<String>,
    },
    #[serde(rename = "resume")]
    Resume,
    #[serde(rename = "request")]
    Request {
        request_seq: u32,
        command: String,
        args: serde_json::Value,
    },
    #[serde(rename = "breakpoints")]
    Breakpoints {
        path: String,
        breakpoints: Vec<u32>,
    },
    #[serde(rename = "minecraftCommand")]
    MinecraftCommand {
        command: String,
        dimension_type: String,
    },
    #[serde(rename = "startProfiler")]
    StartProfiler {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_module_uuid: Option<String>,
    },
    #[serde(rename = "stopProfiler")]
    StopProfiler {
        captures_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_module_uuid: Option<String>,
    },
    #[serde(rename = "stopOnException")]
    StopOnException {
        #[serde(rename = "stopOnException")]
        stop_on_exception: bool,
    },
    #[serde(rename = "debugger-request")]
    DebuggerRequest {
        request_seq: u32,
        request: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_resume() {
        let event = DebuggerEvent::Resume;
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json, serde_json::json!({"type": "resume"}));
        let decoded: DebuggerEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(decoded, DebuggerEvent::Resume));
    }

    #[test]
    fn round_trip_protocol_response() {
        let event = DebuggerEvent::Protocol {
            version: 9,
            target_module_uuid: Some("uuid-123".into()),
            passcode: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "protocol");
        assert_eq!(json["version"], 9);
        assert_eq!(json["target_module_uuid"], "uuid-123");
        assert!(json.get("passcode").is_none());
        let decoded: DebuggerEvent = serde_json::from_value(json).unwrap();
        match decoded {
            DebuggerEvent::Protocol {
                version,
                target_module_uuid,
                passcode,
            } => {
                assert_eq!(version, 9);
                assert_eq!(target_module_uuid.as_deref(), Some("uuid-123"));
                assert!(passcode.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_request() {
        let event = DebuggerEvent::Request {
            request_seq: 7,
            command: "next".into(),
            args: serde_json::json!({"threadId": 0}),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "request");
        assert_eq!(json["request_seq"], 7);
        assert_eq!(json["command"], "next");
        assert_eq!(json["args"]["threadId"], 0);
    }

    #[test]
    fn round_trip_breakpoints() {
        let event = DebuggerEvent::Breakpoints {
            path: "/scripts/main.js".into(),
            breakpoints: vec![10, 20, 30],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "breakpoints");
        assert_eq!(json["path"], "/scripts/main.js");
        assert_eq!(json["breakpoints"], serde_json::json!([10, 20, 30]));
    }

    #[test]
    fn round_trip_minecraft_command() {
        let event = DebuggerEvent::MinecraftCommand {
            command: "/script run test()".into(),
            dimension_type: "overworld".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "minecraftCommand");
        assert_eq!(json["command"], "/script run test()");
        assert_eq!(json["dimension_type"], "overworld");
    }

    #[test]
    fn round_trip_stop_on_exception() {
        let event = DebuggerEvent::StopOnException {
            stop_on_exception: true,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "stopOnException");
        assert_eq!(json["stopOnException"], true);
        let decoded: DebuggerEvent = serde_json::from_value(json).unwrap();
        match decoded {
            DebuggerEvent::StopOnException {
                stop_on_exception,
            } => assert!(stop_on_exception),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_debugger_request() {
        let event = DebuggerEvent::DebuggerRequest {
            request_seq: 99,
            request: "getSource".into(),
            args: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "debugger-request");
        assert_eq!(json["request_seq"], 99);
        assert_eq!(json["request"], "getSource");
        assert!(json.get("args").is_none());
    }
}
