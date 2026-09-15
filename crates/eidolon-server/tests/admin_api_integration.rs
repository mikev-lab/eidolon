//! Integration tests for native HTTP Admin API and Game Master (GM) commands.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use eidolon_core::fixed::Fixed64;
use eidolon_server::EidolonApp;

fn connect_with_retry(addr: std::net::SocketAddr) -> TcpStream {
    for _ in 0..10 {
        if let Ok(s) = TcpStream::connect(addr) {
            return s;
        }
        thread::sleep(Duration::from_millis(10));
    }
    TcpStream::connect(addr).expect("Connect to Admin API")
}

fn http_get(addr: std::net::SocketAddr, path: &str, token: Option<&str>) -> (u16, String) {
    let mut stream = connect_with_retry(addr);
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set_read_timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set_write_timeout");

    let auth_header = match token {
        Some(t) => format!("Authorization: Bearer {}\r\n", t),
        None => String::new(),
    };

    let req = format!(
        "GET {} HTTP/1.1\r\nHost: localhost\r\n{}Connection: close\r\n\r\n",
        path, auth_header
    );
    stream
        .write_all(req.as_bytes())
        .expect("Write HTTP request");

    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    let resp_str = String::from_utf8_lossy(&response).to_string();

    let status_code = resp_str
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);

    (status_code, resp_str)
}

fn http_post(
    addr: std::net::SocketAddr,
    path: &str,
    body: &str,
    token: Option<&str>,
) -> (u16, String) {
    let mut stream = connect_with_retry(addr);
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set_read_timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set_write_timeout");

    let auth_header = match token {
        Some(t) => format!("Authorization: Bearer {}\r\n", t),
        None => String::new(),
    };

    let req = format!(
        "POST {} HTTP/1.1\r\nHost: localhost\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        path,
        auth_header,
        body.len(),
        body
    );
    stream
        .write_all(req.as_bytes())
        .expect("Write HTTP request");

    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    let resp_str = String::from_utf8_lossy(&response).to_string();

    let status_code = resp_str
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);

    (status_code, resp_str)
}

#[test]
fn test_admin_api_security_auth_and_cors() {
    let app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Bind UDP")
        .admin_bind("127.0.0.1:0")
        .expect("Bind Admin TCP")
        .admin_auth_token("secret-token-789".to_string())
        .build()
        .expect("Build EidolonApp");

    let admin_addr = app.admin_local_addr().expect("Admin addr");

    // 1. Unauthenticated request rejected with 401 Unauthorized
    let (code, resp) = http_get(admin_addr, "/api/status", None);
    assert_eq!(code, 401);
    assert!(resp.contains("Unauthorized"));

    // 2. Incorrect token rejected with 401 Unauthorized
    let (code, resp) = http_get(admin_addr, "/api/status", Some("wrong-token"));
    assert_eq!(code, 401);
    assert!(resp.contains("Unauthorized"));

    // 3. Valid bearer token succeeds with 200 OK
    let (code, resp) = http_get(admin_addr, "/api/status", Some("secret-token-789"));
    assert_eq!(code, 200);
    assert!(resp.contains("\"current_tick\":0"));

    // 4. CORS preflight OPTIONS returns 204 No Content
    let mut stream = TcpStream::connect(admin_addr).expect("Connect");
    stream
        .write_all(b"OPTIONS /api/players HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("Write");
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    let resp_str = String::from_utf8_lossy(&response);
    assert!(resp_str.starts_with("HTTP/1.1 204 No Content"));
    assert!(resp_str.contains("Access-Control-Allow-Origin: *"));
}

#[test]
fn test_admin_api_status_and_players_endpoints() {
    let mut app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Bind UDP")
        .admin_bind("127.0.0.1:0")
        .expect("Bind Admin TCP")
        .admin_auth_token("gm-token".to_string())
        .build()
        .expect("Build EidolonApp");

    let admin_addr = app.admin_local_addr().expect("Admin addr");

    // Spawn players
    let peer_addr = "127.0.0.1:54321".parse().unwrap();
    app.spawn_player(1, 10001, 5001, peer_addr, 12.5, 0.0, -8.0)
        .expect("Spawn player 1");
    app.spawn_player(2, 10002, 5002, peer_addr, -30.0, 5.0, 45.0)
        .expect("Spawn player 2");
    app.spawn_npc(99, 1, 0.0, 0.0, 0.0).expect("Spawn NPC 99");

    // Advance 10 ticks so update_admin_snapshot fires
    for _ in 0..10 {
        app.tick().expect("Tick");
    }

    // Check /api/status
    let (code, resp) = http_get(admin_addr, "/api/status", Some("gm-token"));
    assert_eq!(code, 200);
    assert!(resp.contains("\"ccu\":2"));
    assert!(resp.contains("\"total_entities\":3"));
    assert!(resp.contains("\"current_tick\":10"));

    // Check /api/players
    let (code, resp) = http_get(admin_addr, "/api/players", Some("gm-token"));
    assert_eq!(code, 200);
    assert!(resp.contains("\"entity_id\":1"));
    assert!(resp.contains("\"entity_id\":2"));
    assert!(resp.contains("\"account_id\":10001"));
    assert!(resp.contains("\"account_id\":10002"));
    assert!(resp.contains("\"health\":100"));
    assert!(resp.contains("\"x\":12.50"));

    // Check /metrics endpoint
    let (code, resp) = http_get(admin_addr, "/metrics", None);
    assert_eq!(code, 200);
    assert!(resp.contains("eidolon_tick_duration_micros"));
}

#[test]
fn test_admin_api_gm_teleport_kick_and_broadcast() {
    let mut app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Bind UDP")
        .admin_bind("127.0.0.1:0")
        .expect("Bind Admin TCP")
        .admin_auth_token("gm-key".to_string())
        .build()
        .expect("Build EidolonApp");

    let admin_addr = app.admin_local_addr().expect("Admin addr");

    let peer_addr = "127.0.0.1:54322".parse().unwrap();
    app.spawn_player(10, 20001, 6001, peer_addr, 0.0, 0.0, 0.0)
        .expect("Spawn player 10");

    // 1. Teleport player 10 to (250.0, 15.0, -100.0)
    let teleport_body = r#"{"x":250.0,"y":15.0,"z":-100.0}"#;
    let (code, resp) = http_post(
        admin_addr,
        "/api/players/10/teleport",
        teleport_body,
        Some("gm-key"),
    );
    assert_eq!(code, 200);
    assert!(resp.contains("\"success\":true"));

    // Tick server to process command
    app.tick().expect("Tick");

    let p10 = app.get_entity(10).expect("Player 10 exists");
    assert_eq!(p10.position.x, Fixed64::from_f64(250.0));
    assert!((p10.position.y.to_f64() - 15.0).abs() < 0.1);
    assert_eq!(p10.position.z, Fixed64::from_f64(-100.0));

    // 2. Broadcast system announcement
    let broadcast_body = r#"{"text":"World boss will spawn in 5 minutes!"}"#;
    let (code, resp) = http_post(admin_addr, "/api/broadcast", broadcast_body, Some("gm-key"));
    assert_eq!(code, 200);
    assert!(resp.contains("\"success\":true"));

    app.tick().expect("Tick");

    // Verify broadcast event in /api/logs
    let (code, resp) = http_get(admin_addr, "/api/logs", Some("gm-key"));
    assert_eq!(code, 200);
    assert!(resp.contains("World boss will spawn in 5 minutes!"));

    // 3. Kick player 10
    let (code, resp) = http_post(admin_addr, "/api/players/10/kick", "", Some("gm-key"));
    assert_eq!(code, 200);
    assert!(resp.contains("\"success\":true"));

    app.tick().expect("Tick");
    assert!(app.get_entity(10).is_none());

    // 4. Force checkpoint
    let (code, resp) = http_post(admin_addr, "/api/checkpoint", "", Some("gm-key"));
    assert_eq!(code, 200);
    assert!(resp.contains("\"success\":true"));

    app.tick().expect("Tick");
}

#[test]
fn test_admin_api_serves_operator_dashboard() {
    let app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Bind UDP")
        .admin_bind("127.0.0.1:0")
        .expect("Bind Admin TCP")
        .admin_auth_token("secret".to_string())
        .build()
        .expect("Build EidolonApp");

    let admin_addr = app.admin_local_addr().expect("Admin addr");

    // GET / should return 200 OK and HTML even without auth token
    let (code, resp) = http_get(admin_addr, "/", None);
    assert_eq!(code, 200);
    assert!(resp.contains("<!DOCTYPE html>"));
    assert!(resp.contains("EIDOLON"));
    assert!(resp.contains("Mission Control & GM Operations"));
    assert!(resp.contains("Zone Spatial Radar"));

    // GET /dashboard should also serve dashboard
    let (code, resp) = http_get(admin_addr, "/dashboard", None);
    assert_eq!(code, 200);
    assert!(resp.contains("<!DOCTYPE html>"));
}
