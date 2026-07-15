use mc_protocol::{ProtocolVersion, DEFAULT_PORT};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AdapterInfo {
    pub protocol_version: u8,
    pub default_port: u16,
}

pub fn adapter_info() -> AdapterInfo {
    AdapterInfo {
        protocol_version: ProtocolVersion::CURRENT.as_u8(),
        default_port: DEFAULT_PORT,
    }
}
