use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DebuggeeEvent {
    Protocol,
    Stopped,
    Thread,
    Print,
    Stat2,
    ProfilerCapture,
    DebuggeeResponse,
    Schema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DebuggerEvent {
    Resume,
    Step,
    SetBreakpoints,
    MinecraftCommand,
    StartProfiler,
    StopProfiler,
    StopOnException,
    DebuggerRequest,
}
