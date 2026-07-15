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
    serde_json::from_value(value).map_err(Into::into)
}

pub fn encode_debugger_message(event: &DebuggerEvent) -> Result<serde_json::Value, ParseError> {
    serde_json::to_value(event).map_err(Into::into)
}

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

pub fn parse_wrapped_event(_outer: serde_json::Value) -> Result<DebuggeeEvent, ParseError> {
    todo!("wrapped {{type:\"event\", event:{{...}}}} form not yet handled")
}
