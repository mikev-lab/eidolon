//! Native Agones Kubernetes orchestration hooks and lifecycle management.
//!
//! Provides cloud autoscaling coordination, health checking, and graceful termination
//! using native Rust standard library networking without external crates.

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Agones game server pod lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgonesState {
    /// Server is initializing and preparing spatial assets.
    Scheduled,
    /// Server is ready to receive players and participate in matchmaking.
    Ready,
    /// Server has active sessions allocated to connected players.
    Allocated,
    /// Server is shutting down gracefully.
    Shutdown,
}

/// Native Agones SDK client communicating with the local sidecar.
#[derive(Debug)]
pub struct AgonesClient {
    enabled: bool,
    sidecar_port: u16,
    state: AgonesState,
}

impl AgonesClient {
    /// Constructs a new `AgonesClient`.
    ///
    /// When `enabled` is false, runs in standalone/mock mode for local development.
    pub fn new(enabled: bool, sidecar_port: u16) -> Self {
        Self {
            enabled,
            sidecar_port,
            state: AgonesState::Scheduled,
        }
    }

    /// Returns the current lifecycle state of the server.
    #[inline]
    pub fn state(&self) -> AgonesState {
        self.state
    }

    /// Signals the Agones sidecar that the server is initialized and ready for allocations.
    pub fn ready(&mut self) -> Result<(), &'static str> {
        self.send_command("ready")?;
        self.state = AgonesState::Ready;
        Ok(())
    }

    /// Sends a periodic health check ping to the Agones sidecar.
    pub fn health(&self) -> Result<(), &'static str> {
        self.send_command("health")
    }

    /// Signals that the server has been allocated to active player sessions.
    pub fn allocate(&mut self) -> Result<(), &'static str> {
        self.send_command("allocate")?;
        self.state = AgonesState::Allocated;
        Ok(())
    }

    /// Signals that the server is shutting down.
    pub fn shutdown(&mut self) -> Result<(), &'static str> {
        self.send_command("shutdown")?;
        self.state = AgonesState::Shutdown;
        Ok(())
    }

    fn send_command(&self, endpoint: &str) -> Result<(), &'static str> {
        if !self.enabled {
            // Standalone mode: simulated success
            return Ok(());
        }

        let addr = SocketAddr::from(([127, 0, 0, 1], self.sidecar_port));
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(100))
            .map_err(|_| "Failed to connect to Agones sidecar")?;

        let request = format!(
            "POST /{} HTTP/1.1\r\nHost: localhost:{}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}\r\n",
            endpoint, self.sidecar_port
        );

        stream
            .write_all(request.as_bytes())
            .map_err(|_| "Failed to send command to Agones sidecar")?;

        Ok(())
    }
}

impl Default for AgonesClient {
    fn default() -> Self {
        Self::new(false, 9358)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agones_standalone_mock_lifecycle() {
        let mut client = AgonesClient::new(false, 9358);
        assert_eq!(client.state(), AgonesState::Scheduled);

        assert!(client.ready().is_ok());
        assert_eq!(client.state(), AgonesState::Ready);

        assert!(client.health().is_ok());

        assert!(client.allocate().is_ok());
        assert_eq!(client.state(), AgonesState::Allocated);

        assert!(client.shutdown().is_ok());
        assert_eq!(client.state(), AgonesState::Shutdown);
    }

    #[test]
    fn test_agones_tcp_live_sidecar_ipc() {
        use std::io::Read;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local addr").port();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0u8; 256];
            let n = stream.read(&mut buf).expect("read stream");
            String::from_utf8_lossy(&buf[..n]).to_string()
        });

        let mut client = AgonesClient::new(true, port);
        assert!(client.ready().is_ok());
        assert_eq!(client.state(), AgonesState::Ready);

        let received = handle.join().expect("join thread");
        assert!(received.starts_with("POST /ready HTTP/1.1\r\n"));
        assert!(received.contains(&format!("Host: localhost:{port}\r\n")));
    }
}
