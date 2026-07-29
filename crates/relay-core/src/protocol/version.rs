use super::{Capabilities, CURRENT_PROTOCOL_VERSION};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientProtocolRange {
    pub min: u16,
    pub max: u16,
}

impl Default for ClientProtocolRange {
    fn default() -> Self {
        Self {
            min: CURRENT_PROTOCOL_VERSION,
            max: CURRENT_PROTOCOL_VERSION,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NegotiatedProtocol {
    pub version: u16,
    pub server_id: String,
    pub identity_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    InvalidClientRange,
    Incompatible {
        client_min: u16,
        client_max: u16,
        server_version: u16,
        server_min_client: u16,
    },
    InvalidServerIdentity,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClientRange => formatter.write_str("invalid client protocol range"),
            Self::Incompatible { .. } => formatter.write_str("server protocol is incompatible"),
            Self::InvalidServerIdentity => formatter.write_str("server identity is invalid"),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub fn negotiate(
    client: ClientProtocolRange,
    server: &Capabilities,
) -> Result<NegotiatedProtocol, ProtocolError> {
    if client.min == 0 || client.max < client.min {
        return Err(ProtocolError::InvalidClientRange);
    }
    if server.server_id.trim().is_empty() || server.identity_fingerprint.trim().is_empty() {
        return Err(ProtocolError::InvalidServerIdentity);
    }
    let compatible = server.protocol_version >= client.min
        && server.protocol_version <= client.max
        && server.compatibility_min_client <= client.max;
    if !compatible {
        return Err(ProtocolError::Incompatible {
            client_min: client.min,
            client_max: client.max,
            server_version: server.protocol_version,
            server_min_client: server.compatibility_min_client,
        });
    }
    Ok(NegotiatedProtocol {
        version: server.protocol_version,
        server_id: server.server_id.clone(),
        identity_fingerprint: server.identity_fingerprint.clone(),
    })
}
