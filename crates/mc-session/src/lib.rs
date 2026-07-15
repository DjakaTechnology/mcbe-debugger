use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("not connected to minecraft")]
    NotConnected,
    #[error("protocol version mismatch: client {client}, server {server}")]
    ProtocolVersionMismatch { client: u8, server: u8 },
    #[error("transport error: {0}")]
    Transport(String),
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Stopped { thread_id: u32 },
    Thread { thread_id: u32 },
    Print { output: String },
    Disconnected,
}

#[async_trait]
pub trait DebugSession: Send + Sync {
    async fn resume(&self, thread_id: u32) -> Result<(), SessionError>;
    async fn pause(&self, thread_id: u32) -> Result<(), SessionError>;
}
