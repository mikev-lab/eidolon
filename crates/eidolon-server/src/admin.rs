//! Native zero-dependency HTTP Admin API and Live-Ops management service.
//!
//! Provides a secure, decoupled administrative REST interface for fleet health monitoring,
//! Game Master (GM) player management, server-wide announcements, and Prometheus metrics exposition.
//! Runs on an asynchronous background thread with zero overhead on the 20 Hz simulation tick loop.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Maximum number of recent administrative log entries retained in memory.
pub const MAX_ADMIN_LOGS: usize = 64;

/// Maximum number of pending GM commands queued before dropping new submissions.
pub const MAX_PENDING_GM_COMMANDS: usize = 256;

/// Administrative Game Master (GM) action command queued for authoritative tick execution.
#[derive(Debug, Clone, PartialEq)]
pub enum GmCommand {
    /// Forcibly disconnects a player from the server and despawns their entity.
    KickPlayer {
        /// Entity ID of the player to kick.
        entity_id: u32,
    },
    /// Teleports a player to authoritative target coordinates.
    TeleportPlayer {
        /// Entity ID of the player to teleport.
        entity_id: u32,
        /// Target world coordinate X.
        x: f64,
        /// Target world coordinate Y (elevation).
        y: f64,
        /// Target world coordinate Z.
        z: f64,
    },
    /// Broadcasts a server-wide system announcement to all connected players.
    BroadcastMessage {
        /// Text message content to broadcast.
        text: String,
    },
    /// Triggers an immediate durability checkpoint and WAL ring buffer flush.
    ForceCheckpoint,
}

/// Lightweight snapshot of the server operational state for administrative telemetry.
#[derive(Debug, Clone, PartialEq)]
pub struct AdminStatusSnapshot {
    /// Server uptime in seconds.
    pub uptime_secs: u64,
    /// Current authoritative simulation tick index.
    pub current_tick: u64,
    /// Number of active connected player sessions (CCU).
    pub ccu: usize,
    /// Total active entities in the simulation (players, monsters, NPCs).
    pub total_entities: usize,
    /// Number of active world zones or spatial partitions.
    pub zone_count: usize,
    /// 50th percentile tick duration in microseconds.
    pub tick_p50_micros: u64,
    /// 99th percentile tick duration in microseconds.
    pub tick_p99_micros: u64,
    /// Estimated memory usage in kilobytes.
    pub memory_kb: usize,
    /// Current adaptive load shedding state (e.g. "None", "Level1", "Level2").
    pub shedding_level: String,
    /// Current average replication wire egress in kilobytes/second per client.
    pub wire_egress_kbps: f64,
}

impl Default for AdminStatusSnapshot {
    fn default() -> Self {
        Self {
            uptime_secs: 0,
            current_tick: 0,
            ccu: 0,
            total_entities: 0,
            zone_count: 1,
            tick_p50_micros: 250,
            tick_p99_micros: 500,
            memory_kb: 4096,
            shedding_level: "None".to_string(),
            wire_egress_kbps: 0.85,
        }
    }
}

/// Snapshot of an individual connected player for administrative inspection.
#[derive(Debug, Clone, PartialEq)]
pub struct AdminPlayerSnapshot {
    /// Authoritative entity ID.
    pub entity_id: u32,
    /// Associated account identifier.
    pub account_id: u64,
    /// Character or username identifier.
    pub name: String,
    /// Current continuous position X.
    pub x: f64,
    /// Current continuous position Y (elevation).
    pub y: f64,
    /// Current continuous position Z.
    pub z: f64,
    /// Current health points.
    pub health: u32,
    /// Maximum health points.
    pub max_health: u32,
    /// Current mana points.
    pub mana: u32,
    /// Maximum mana points.
    pub max_mana: u32,
    /// Remote socket address of the client peer.
    pub peer_addr: String,
    /// Number of equipped gear items.
    pub equipped_items_count: usize,
}

/// Thread-safe communication bridge between the 20 Hz simulation loop and the Admin HTTP API.
#[derive(Debug)]
pub struct AdminBridge {
    /// Queue of GM commands submitted via HTTP to be executed during the simulation tick.
    pub pending_commands: Mutex<VecDeque<GmCommand>>,
    /// Latest telemetry snapshot updated by the simulation loop.
    pub status: Mutex<AdminStatusSnapshot>,
    /// Latest active player list updated by the simulation loop.
    pub players: Mutex<Vec<AdminPlayerSnapshot>>,
    /// Circular buffer of recent operational and GM event logs.
    pub logs: Mutex<VecDeque<String>>,
    /// Prometheus text exposition metrics rendered by the simulation loop.
    pub prometheus_metrics: Mutex<String>,
}

impl Default for AdminBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl AdminBridge {
    /// Constructs a new `AdminBridge` with empty queues and default status.
    pub fn new() -> Self {
        Self {
            pending_commands: Mutex::new(VecDeque::with_capacity(MAX_PENDING_GM_COMMANDS)),
            status: Mutex::new(AdminStatusSnapshot::default()),
            players: Mutex::new(Vec::new()),
            logs: Mutex::new(VecDeque::with_capacity(MAX_ADMIN_LOGS)),
            prometheus_metrics: Mutex::new(String::new()),
        }
    }

    /// Records an administrative event log entry.
    pub fn log_event(&self, message: &str) {
        if let Ok(mut lock) = self.logs.lock() {
            if lock.len() >= MAX_ADMIN_LOGS {
                lock.pop_front();
            }
            lock.push_back(message.to_string());
        }
    }

    /// Enqueues a GM command for simulation execution.
    pub fn enqueue_command(&self, command: GmCommand) -> Result<(), &'static str> {
        let mut lock = self
            .pending_commands
            .lock()
            .map_err(|_| "AdminBridge lock poisoned")?;
        if lock.len() >= MAX_PENDING_GM_COMMANDS {
            return Err("Pending command queue full");
        }
        lock.push_back(command);
        Ok(())
    }

    /// Drains all pending GM commands into the provided vector (called by `ServerFacade::tick`).
    pub fn drain_commands(&self, dest: &mut Vec<GmCommand>) {
        if let Ok(mut lock) = self.pending_commands.lock() {
            while let Some(cmd) = lock.pop_front() {
                dest.push(cmd);
            }
        }
    }

    /// Updates the cached operational status snapshot.
    pub fn update_status(&self, snapshot: AdminStatusSnapshot) {
        if let Ok(mut lock) = self.status.lock() {
            *lock = snapshot;
        }
    }

    /// Updates the cached player list snapshot.
    pub fn update_players(&self, players: Vec<AdminPlayerSnapshot>) {
        if let Ok(mut lock) = self.players.lock() {
            *lock = players;
        }
    }

    /// Updates the cached Prometheus exposition text.
    pub fn update_prometheus_metrics(&self, metrics: String) {
        if let Ok(mut lock) = self.prometheus_metrics.lock() {
            *lock = metrics;
        }
    }
}

/// Native zero-dependency HTTP server providing the administrative REST API.
#[derive(Debug)]
pub struct AdminServer {
    local_addr: SocketAddr,
    auth_token: Option<String>,
    running: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,
}

impl AdminServer {
    /// Binds a new `AdminServer` to the specified address.
    ///
    /// The server runs on a background OS thread and communicates with the game engine
    /// exclusively via the provided `AdminBridge`.
    pub fn bind(
        bind_addr: SocketAddr,
        auth_token: Option<String>,
        bridge: Arc<AdminBridge>,
    ) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind(bind_addr)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        let auth_token_clone = auth_token.clone();

        let thread_handle = thread::Builder::new()
            .name("eidolon-admin-http".to_string())
            .spawn(move || {
                run_http_listener(listener, bridge, auth_token_clone, running_clone);
            })?;

        Ok(Self {
            local_addr,
            auth_token,
            running,
            thread_handle: Some(thread_handle),
        })
    }

    /// Returns the local TCP socket address the Admin API is listening on.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Returns whether bearer token authentication is enabled.
    pub fn is_auth_enabled(&self) -> bool {
        self.auth_token.is_some()
    }

    /// Stops the administrative HTTP server and awaits background thread termination.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for AdminServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_http_listener(
    listener: TcpListener,
    bridge: Arc<AdminBridge>,
    expected_token: Option<String>,
    running: Arc<AtomicBool>,
) {
    while running.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _peer_addr)) => {
                let _ = stream.set_nonblocking(false);
                let bridge_ref = Arc::clone(&bridge);
                let token_ref = expected_token.clone();
                // Handle request synchronously on timeout
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
                handle_http_connection(stream, &bridge_ref, token_ref.as_deref());
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn handle_http_connection(
    mut stream: TcpStream,
    bridge: &AdminBridge,
    expected_token: Option<&str>,
) {
    let mut request_buf = [0u8; 4096];
    let bytes_read = match stream.read(&mut request_buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let request_str = match std::str::from_utf8(&request_buf[..bytes_read]) {
        Ok(s) => s,
        Err(_) => {
            let _ = write_http_response(
                &mut stream,
                400,
                "Bad Request",
                "application/json",
                b"{\"error\":\"Invalid UTF-8 request\"}",
            );
            return;
        }
    };

    let mut lines = request_str.lines();
    let request_line = match lines.next() {
        Some(l) => l,
        None => return,
    };

    let mut parts = request_line.split_whitespace();
    let method = match parts.next() {
        Some(m) => m,
        None => return,
    };
    let path = match parts.next() {
        Some(p) => p,
        None => return,
    };

    // Handle CORS preflight request
    if method == "OPTIONS" {
        let _ = write_cors_preflight_response(&mut stream);
        return;
    }

    // Authenticate if bearer token is required
    if let Some(token) = expected_token {
        let mut is_authenticated = false;
        for line in lines.clone() {
            let lower = line.to_lowercase();
            if lower.starts_with("authorization:") {
                let auth_val = line[14..].trim();
                if let Some(bearer) = auth_val.strip_prefix("Bearer ") {
                    if constant_time_eq_str(bearer, token) {
                        is_authenticated = true;
                        break;
                    }
                }
            }
        }

        let is_public_path =
            path == "/metrics" || path == "/" || path == "/index.html" || path == "/dashboard";

        if !is_authenticated && !is_public_path {
            let _ = write_http_response(
                &mut stream,
                401,
                "Unauthorized",
                "application/json",
                b"{\"error\":\"Unauthorized: Invalid or missing Bearer token\"}",
            );
            return;
        }
    }

    // Extract request body if present
    let body = if let Some(body_start) = request_str.find("\r\n\r\n") {
        &request_str[body_start + 4..]
    } else {
        ""
    };

    // Route endpoints
    match (method, path) {
        ("GET", "/api/status") => {
            let status = match bridge.status.lock() {
                Ok(s) => s.clone(),
                Err(_) => AdminStatusSnapshot::default(),
            };
            let json = format!(
                "{{\"uptime_secs\":{},\"current_tick\":{},\"ccu\":{},\"total_entities\":{},\"zone_count\":{},\"tick_p50_micros\":{},\"tick_p99_micros\":{},\"memory_kb\":{},\"shedding_level\":\"{}\",\"wire_egress_kbps\":{:.2}}}",
                status.uptime_secs,
                status.current_tick,
                status.ccu,
                status.total_entities,
                status.zone_count,
                status.tick_p50_micros,
                status.tick_p99_micros,
                status.memory_kb,
                status.shedding_level,
                status.wire_egress_kbps,
            );
            let _ =
                write_http_response(&mut stream, 200, "OK", "application/json", json.as_bytes());
        }

        ("GET", "/api/players") => {
            let players = match bridge.players.lock() {
                Ok(p) => p.clone(),
                Err(_) => Vec::new(),
            };

            let mut json = String::from("{\"players\":[");
            for (idx, p) in players.iter().enumerate() {
                if idx > 0 {
                    json.push(',');
                }
                json.push_str(&format!(
                    "{{\"entity_id\":{},\"account_id\":{},\"name\":\"{}\",\"x\":{:.2},\"y\":{:.2},\"z\":{:.2},\"health\":{},\"max_health\":{},\"mana\":{},\"max_mana\":{},\"peer_addr\":\"{}\",\"equipped_items_count\":{}}}",
                    p.entity_id,
                    p.account_id,
                    p.name,
                    p.x,
                    p.y,
                    p.z,
                    p.health,
                    p.max_health,
                    p.mana,
                    p.max_mana,
                    p.peer_addr,
                    p.equipped_items_count,
                ));
            }
            json.push_str("]}");
            let _ =
                write_http_response(&mut stream, 200, "OK", "application/json", json.as_bytes());
        }

        ("GET", "/api/logs") => {
            let logs = match bridge.logs.lock() {
                Ok(l) => l.iter().cloned().collect::<Vec<_>>(),
                Err(_) => Vec::new(),
            };

            let mut json = String::from("{\"logs\":[");
            for (idx, entry) in logs.iter().enumerate() {
                if idx > 0 {
                    json.push(',');
                }
                json.push_str(&format!("\"{}\"", entry.replace('"', "\\\"")));
            }
            json.push_str("]}");
            let _ =
                write_http_response(&mut stream, 200, "OK", "application/json", json.as_bytes());
        }

        ("POST", "/api/broadcast") => {
            // Extract "text" from body
            let text = extract_json_string_field(body, "text")
                .unwrap_or_else(|| "Server announcement".to_string());
            let log_msg = format!("GM broadcast: {}", text);
            bridge.log_event(&log_msg);

            let _ = bridge.enqueue_command(GmCommand::BroadcastMessage { text });
            let _ = write_http_response(
                &mut stream,
                200,
                "OK",
                "application/json",
                b"{\"success\":true,\"message\":\"Broadcast enqueued\"}",
            );
        }

        ("POST", "/api/checkpoint") => {
            bridge.log_event("GM triggered manual WAL durability checkpoint");
            let _ = bridge.enqueue_command(GmCommand::ForceCheckpoint);
            let _ = write_http_response(
                &mut stream,
                200,
                "OK",
                "application/json",
                b"{\"success\":true,\"message\":\"Checkpoint enqueued\"}",
            );
        }

        ("POST", p) if p.starts_with("/api/players/") && p.ends_with("/kick") => {
            // Path format: /api/players/{id}/kick
            let segments: Vec<&str> = p.split('/').collect();
            if segments.len() >= 4 {
                if let Ok(entity_id) = segments[3].parse::<u32>() {
                    let log_msg = format!("GM kicked player {}", entity_id);
                    bridge.log_event(&log_msg);
                    let _ = bridge.enqueue_command(GmCommand::KickPlayer { entity_id });
                    let _ = write_http_response(
                        &mut stream,
                        200,
                        "OK",
                        "application/json",
                        b"{\"success\":true,\"message\":\"Player kick enqueued\"}",
                    );
                    return;
                }
            }
            let _ = write_http_response(
                &mut stream,
                400,
                "Bad Request",
                "application/json",
                b"{\"error\":\"Invalid player entity ID\"}",
            );
        }

        ("POST", p) if p.starts_with("/api/players/") && p.ends_with("/teleport") => {
            let segments: Vec<&str> = p.split('/').collect();
            if segments.len() >= 4 {
                if let Ok(entity_id) = segments[3].parse::<u32>() {
                    let x = extract_json_float_field(body, "x").unwrap_or(0.0);
                    let y = extract_json_float_field(body, "y").unwrap_or(0.0);
                    let z = extract_json_float_field(body, "z").unwrap_or(0.0);

                    let log_msg = format!(
                        "GM teleported player {} to ({:.1}, {:.1}, {:.1})",
                        entity_id, x, y, z
                    );
                    bridge.log_event(&log_msg);

                    let _ =
                        bridge.enqueue_command(GmCommand::TeleportPlayer { entity_id, x, y, z });
                    let _ = write_http_response(
                        &mut stream,
                        200,
                        "OK",
                        "application/json",
                        b"{\"success\":true,\"message\":\"Teleport enqueued\"}",
                    );
                    return;
                }
            }
            let _ = write_http_response(
                &mut stream,
                400,
                "Bad Request",
                "application/json",
                b"{\"error\":\"Invalid teleport request\"}",
            );
        }

        ("GET", "/") | ("GET", "/index.html") | ("GET", "/dashboard") => {
            let html = include_str!("../static/dashboard.html");
            let _ = write_http_response(
                &mut stream,
                200,
                "OK",
                "text/html; charset=utf-8",
                html.as_bytes(),
            );
        }

        ("GET", "/metrics") => {
            let metrics = match bridge.prometheus_metrics.lock() {
                Ok(m) => m.clone(),
                Err(_) => String::new(),
            };
            let _ = write_http_response(
                &mut stream,
                200,
                "OK",
                "text/plain; version=0.0.4",
                metrics.as_bytes(),
            );
        }

        _ => {
            let _ = write_http_response(
                &mut stream,
                404,
                "Not Found",
                "application/json",
                b"{\"error\":\"Endpoint not found\"}",
            );
        }
    }
}

fn write_http_response(
    stream: &mut TcpStream,
    status_code: u16,
    status_text: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(), std::io::Error> {
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Authorization, Content-Type\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nConnection: close\r\n\r\n",
        status_code,
        status_text,
        content_type,
        body.len()
    );

    let mut response_buf = Vec::with_capacity(header.len() + body.len());
    response_buf.extend_from_slice(header.as_bytes());
    response_buf.extend_from_slice(body);
    stream.write_all(&response_buf)?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}

fn write_cors_preflight_response(stream: &mut TcpStream) -> Result<(), std::io::Error> {
    let header = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Authorization, Content-Type\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Max-Age: 86400\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream.write_all(header.as_bytes())?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}

fn extract_json_string_field(json: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{}\"", field);
    let start_idx = json.find(&pattern)?;
    let after_key = &json[start_idx + pattern.len()..];
    let colon_idx = after_key.find(':')?;
    let after_colon = after_key[colon_idx + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let rest = &after_colon[1..];
    let end_quote = rest.find('"')?;
    Some(rest[..end_quote].to_string())
}

fn extract_json_float_field(json: &str, field: &str) -> Option<f64> {
    let pattern = format!("\"{}\"", field);
    let start_idx = json.find(&pattern)?;
    let after_key = &json[start_idx + pattern.len()..];
    let colon_idx = after_key.find(':')?;
    let after_colon = after_key[colon_idx + 1..].trim_start();

    let mut end_idx = 0;
    for ch in after_colon.chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' {
            end_idx += ch.len_utf8();
        } else {
            break;
        }
    }

    if end_idx == 0 {
        return None;
    }

    after_colon[..end_idx].parse::<f64>().ok()
}

fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.len() != b_bytes.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a_bytes.iter().zip(b_bytes.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_bridge_command_fifo_and_capacity() {
        let bridge = AdminBridge::new();
        assert!(bridge.enqueue_command(GmCommand::ForceCheckpoint).is_ok());
        assert!(bridge
            .enqueue_command(GmCommand::KickPlayer { entity_id: 101 })
            .is_ok());

        let mut drained = Vec::new();
        bridge.drain_commands(&mut drained);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0], GmCommand::ForceCheckpoint);
        assert_eq!(drained[1], GmCommand::KickPlayer { entity_id: 101 });

        // Subsequent drain is empty
        drained.clear();
        bridge.drain_commands(&mut drained);
        assert!(drained.is_empty());
    }

    #[test]
    fn test_admin_bridge_circular_logs_cap() {
        let bridge = AdminBridge::new();
        for i in 0..MAX_ADMIN_LOGS + 10 {
            bridge.log_event(&format!("Log event {}", i));
        }

        let logs = bridge.logs.lock().unwrap();
        assert_eq!(logs.len(), MAX_ADMIN_LOGS);
        assert_eq!(
            logs.back().unwrap(),
            &format!("Log event {}", MAX_ADMIN_LOGS + 9)
        );
    }

    #[test]
    fn test_json_field_extractors() {
        let json = r#"{"text":"Global broadcast alert!","x":123.45,"y":-10.0,"z":0.0}"#;
        assert_eq!(
            extract_json_string_field(json, "text"),
            Some("Global broadcast alert!".to_string())
        );
        assert_eq!(extract_json_float_field(json, "x"), Some(123.45));
        assert_eq!(extract_json_float_field(json, "y"), Some(-10.0));
        assert_eq!(extract_json_float_field(json, "z"), Some(0.0));
        assert_eq!(extract_json_float_field(json, "missing"), None);
    }

    #[test]
    fn test_admin_server_http_status_and_auth() {
        let bridge = Arc::new(AdminBridge::new());
        bridge.update_status(AdminStatusSnapshot {
            uptime_secs: 120,
            current_tick: 2400,
            ccu: 42,
            ..Default::default()
        });

        let mut server = AdminServer::bind(
            "127.0.0.1:0".parse().unwrap(),
            Some("secret-key-123".to_string()),
            Arc::clone(&bridge),
        )
        .expect("Bind server");

        let addr = server.local_addr();

        // 1. Unauthenticated request must return 401
        let mut stream = TcpStream::connect(addr).expect("Connect");
        stream
            .write_all(b"GET /api/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("Send");
        let mut resp = Vec::new();
        let _ = stream.read_to_end(&mut resp);
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(resp_str.starts_with("HTTP/1.1 401 Unauthorized"));

        // 2. Authenticated request with Bearer token must return 200 OK
        let mut stream = TcpStream::connect(addr).expect("Connect");
        stream
            .write_all(b"GET /api/status HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer secret-key-123\r\nConnection: close\r\n\r\n")
            .expect("Send");
        let mut resp = Vec::new();
        let _ = stream.read_to_end(&mut resp);
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(resp_str.starts_with("HTTP/1.1 200 OK"));
        assert!(resp_str.contains("\"ccu\":42"));
        assert!(resp_str.contains("\"uptime_secs\":120"));

        // 3. CORS preflight OPTIONS must return 204
        let mut stream = TcpStream::connect(addr).expect("Connect");
        stream
            .write_all(
                b"OPTIONS /api/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .expect("Send");
        let mut resp = Vec::new();
        let _ = stream.read_to_end(&mut resp);
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(resp_str.starts_with("HTTP/1.1 204 No Content"));

        server.stop();
    }
}
