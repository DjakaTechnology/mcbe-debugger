pub mod events;
pub mod framing;
pub mod version;

pub const DEFAULT_PORT: u16 = 19144;

pub use events::{DebuggeeEvent, DebuggerEvent};
pub use framing::MessageCodec;
pub use version::ProtocolVersion;
