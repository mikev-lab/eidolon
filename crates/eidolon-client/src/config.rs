//! Client configuration and connection parameters.

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use eidolon_core::identity::{AccountId, SessionTicket};

/// Configuration parameters for establishing and maintaining an `EidolonClient` session.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Authoritative server address to connect to.
    pub server_addr: SocketAddr,
    /// Account identifier for authentication.
    pub account_id: AccountId,
    /// Active session ticket credential.
    pub session_ticket: SessionTicket,
    /// Connection and response timeout duration.
    pub timeout: Duration,
    /// Client-side interpolation delay in milliseconds (typically 50ms to 100ms).
    pub interpolation_delay_ms: u32,
    /// Optional local socket address to bind to (defaults to `0.0.0.0:0`).
    pub local_bind_addr: Option<SocketAddr>,
}

impl ClientConfig {
    /// Creates a new client configuration with default timeouts and interpolation window.
    pub fn new<A: ToSocketAddrs>(
        server_addr: A,
        account_id: AccountId,
        session_ticket: SessionTicket,
    ) -> Result<Self, std::io::Error> {
        let addr = server_addr.to_socket_addrs()?.next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid server address")
        })?;

        Ok(Self {
            server_addr: addr,
            account_id,
            session_ticket,
            timeout: Duration::from_secs(5),
            interpolation_delay_ms: 50,
            local_bind_addr: None,
        })
    }
}

impl Default for ClientConfig {
    fn default() -> Self {
        use std::net::{IpAddr, Ipv4Addr};
        Self {
            server_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7777),
            account_id: AccountId(0),
            session_ticket: SessionTicket([0u8; 16]),
            timeout: Duration::from_secs(5),
            interpolation_delay_ms: 50,
            local_bind_addr: None,
        }
    }
}
