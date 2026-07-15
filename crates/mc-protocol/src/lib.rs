pub mod connection;
pub mod events;
pub mod framing;
pub mod version;

pub const DEFAULT_PORT: u16 = 19144;

pub use connection::{
    ConnectOptions, ConnectionError, DebuggeeConnection, DebuggeeResponse, ProtocolHandshake,
};
pub use events::{DebuggeeEvent, DebuggerEvent};
pub use framing::MessageCodec;
pub use version::ProtocolVersion;
