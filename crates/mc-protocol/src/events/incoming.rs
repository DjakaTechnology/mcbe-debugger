use serde::{Deserialize, Serialize};

use crate::events::shared::{
    DiagnosticsTabDescriptor, LogLevel, PluginDetails, StatDataModel,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DebuggeeEvent {
    #[serde(rename = "ProtocolEvent")]
    Protocol {
        version: u8,
        plugins: Vec<PluginDetails>,
        #[serde(default)]
        require_passcode: bool,
    },
    #[serde(rename = "StoppedEvent")]
    Stopped {
        reason: String,
        thread: u32,
    },
    #[serde(rename = "ThreadEvent")]
    Thread {
        reason: String,
        thread: u32,
    },
    #[serde(rename = "PrintEvent")]
    Print {
        message: String,
        #[serde(rename = "logLevel")]
        log_level: LogLevel,
    },
    #[serde(rename = "NotificationEvent")]
    Notification {
        message: String,
        #[serde(rename = "logLevel")]
        log_level: LogLevel,
    },
    #[serde(rename = "StatEvent2")]
    Stat2 {
        tick: u64,
        stats: Vec<StatDataModel>,
    },
    #[serde(rename = "ProfilerCapture")]
    ProfilerCapture {
        capture_base_path: String,
        capture_data: String,
    },
    #[serde(rename = "debuggee-response")]
    DebuggeeResponse {
        request_seq: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        success: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_message: Option<String>,
    },
    #[serde(rename = "response")]
    Response {
        request_seq: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        success: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(rename = "SchemaEvent")]
    Schema {
        descriptors: Vec<DiagnosticsTabDescriptor>,
    },
    #[serde(rename = "terminated")]
    Terminated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Unknown {
        type_name: String,
        #[serde(skip)]
        data: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::shared::DiagnosticsDataSource;

    #[test]
    fn round_trip_protocol_event() {
        let event = DebuggeeEvent::Protocol {
            version: 9,
            plugins: vec![PluginDetails {
                name: "test".into(),
                module_uuid: "abc-123".into(),
            }],
            require_passcode: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "ProtocolEvent");
        assert_eq!(json["version"], 9);
        let decoded: DebuggeeEvent = serde_json::from_value(json).unwrap();
        match decoded {
            DebuggeeEvent::Protocol {
                version,
                plugins,
                require_passcode,
            } => {
                assert_eq!(version, 9);
                assert_eq!(plugins.len(), 1);
                assert!(!require_passcode);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_stopped_event() {
        let event = DebuggeeEvent::Stopped {
            reason: "breakpoint".into(),
            thread: 0,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "StoppedEvent");
        assert_eq!(json["reason"], "breakpoint");
        assert_eq!(json["thread"], 0);
        let decoded: DebuggeeEvent = serde_json::from_value(json).unwrap();
        match decoded {
            DebuggeeEvent::Stopped { reason, thread } => {
                assert_eq!(reason, "breakpoint");
                assert_eq!(thread, 0);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_print_event() {
        let event = DebuggeeEvent::Print {
            message: "hello".into(),
            log_level: LogLevel::Warn,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "PrintEvent");
        assert_eq!(json["logLevel"], 1);
        let decoded: DebuggeeEvent = serde_json::from_value(json).unwrap();
        match decoded {
            DebuggeeEvent::Print { message, log_level } => {
                assert_eq!(message, "hello");
                assert_eq!(log_level, LogLevel::Warn);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_debuggee_response() {
        let event = DebuggeeEvent::DebuggeeResponse {
            request_seq: 42,
            args: None,
            success: Some(false),
            response_message: Some("nope".into()),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "debuggee-response");
        assert_eq!(json["request_seq"], 42);
        assert_eq!(json["success"], false);
        let decoded: DebuggeeEvent = serde_json::from_value(json).unwrap();
        match decoded {
            DebuggeeEvent::DebuggeeResponse {
                request_seq,
                success,
                ..
            } => {
                assert_eq!(request_seq, 42);
                assert_eq!(success, Some(false));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_schema_event() {
        let event = DebuggeeEvent::Schema {
            descriptors: vec![DiagnosticsTabDescriptor {
                name: "Server Timing".into(),
                stat_group_id: "ServerTiming".into(),
                data_source: DiagnosticsDataSource::Server,
                display_type: crate::events::shared::DiagnosticsDisplayType::LineChart,
                title: None,
                y_label: None,
                tick_range: None,
                value_scalar: None,
                target_value: None,
                key_label: None,
                value_labels: None,
                statistic_id: None,
                statistic_ids: None,
            }],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "SchemaEvent");
        assert_eq!(json["descriptors"][0]["data_source"], "server");
        assert_eq!(json["descriptors"][0]["display_type"], "line_chart");
    }

    #[test]
    fn deserialize_minimal_protocol_event() {
        let json = serde_json::json!({
            "type": "ProtocolEvent",
            "version": 9,
            "plugins": []
        });
        let event: DebuggeeEvent = serde_json::from_value(json).unwrap();
        match event {
            DebuggeeEvent::Protocol {
                version,
                plugins,
                require_passcode,
            } => {
                assert_eq!(version, 9);
                assert!(plugins.is_empty());
                assert!(!require_passcode);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn deserialize_unknown_variant_errors() {
        let json = serde_json::json!({"type": "UnknownThing", "data": 1});
        let result: Result<DebuggeeEvent, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }
}
