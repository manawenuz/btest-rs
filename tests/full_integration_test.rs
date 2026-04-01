//! Comprehensive integration tests covering all modes, protocols, and output formats.
//! Each test starts a server, runs a client, verifies data flows, and checks CSV/stats.

use std::net::UdpSocket as StdUdpSocket;
use std::sync::atomic::Ordering;
use std::time::Duration;

const BASE_PORT: u16 = 14000;

// --- Helpers ---

async fn start_server(port: u16, ecsrp5: bool) {
    let auth_user = Some("testuser".into());
    let auth_pass = Some("testpass".into());
    tokio::spawn(async move {
        let _ = btest_rs::server::run_server(
            port, auth_user, auth_pass, ecsrp5,
            Some("127.0.0.1".into()), None,
        ).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
}

async fn start_server_noauth(port: u16) {
    tokio::spawn(async move {
        let _ = btest_rs::server::run_server(
            port, None, None, false,
            Some("127.0.0.1".into()), None,
        ).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
}

async fn start_server_v6(port: u16) {
    tokio::spawn(async move {
        let _ = btest_rs::server::run_server(
            port, None, None, false,
            None, Some("::1".into()),
        ).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
}

async fn run_client_test(
    host: &str, port: u16, transmit: bool, receive: bool, udp: bool,
    user: Option<&str>, pass: Option<&str>,
) -> (u64, u64, u64, u32) {
    let direction = match (transmit, receive) {
        (true, false) => btest_rs::protocol::CMD_DIR_RX,
        (false, true) => btest_rs::protocol::CMD_DIR_TX,
        (true, true) => btest_rs::protocol::CMD_DIR_BOTH,
        _ => panic!("must specify direction"),
    };
    let state = btest_rs::bandwidth::BandwidthState::new();
    let state_clone = state.clone();

    let host = host.to_string();
    let user = user.map(String::from);
    let pass = pass.map(String::from);

    let handle = tokio::spawn(async move {
        btest_rs::client::run_client(
            &host, port, direction, udp,
            0, 0, user, pass, false, state_clone,
        ).await
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    state.running.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(500)).await;
    handle.abort();

    state.summary()
}

// --- TCP IPv4 Tests ---

#[tokio::test]
async fn test_tcp4_receive() {
    let port = BASE_PORT;
    start_server_noauth(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("127.0.0.1", port, false, true, false, None, None).await;
    assert!(_rx > 0, "TCP4 receive: expected rx > 0, got {}", _rx);
    assert!(_intervals > 0, "TCP4 receive: expected intervals > 0");
}

#[tokio::test]
async fn test_tcp4_send() {
    let port = BASE_PORT + 1;
    start_server_noauth(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("127.0.0.1", port, true, false, false, None, None).await;
    assert!(_tx > 0, "TCP4 send: expected tx > 0, got {}", _tx);
    assert!(_intervals > 0, "TCP4 send: expected intervals > 0");
}

#[tokio::test]
async fn test_tcp4_both() {
    let port = BASE_PORT + 2;
    start_server_noauth(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("127.0.0.1", port, true, true, false, None, None).await;
    assert!(_tx > 0, "TCP4 both: expected tx > 0, got {}", _tx);
    assert!(_rx > 0, "TCP4 both: expected rx > 0, got {}", _rx);
}

// --- UDP IPv4 Tests ---

#[tokio::test]
async fn test_udp4_receive() {
    let port = BASE_PORT + 3;
    start_server_noauth(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("127.0.0.1", port, false, true, true, None, None).await;
    assert!(_rx > 0, "UDP4 receive: expected rx > 0, got {}", _rx);
}

#[tokio::test]
async fn test_udp4_send() {
    let port = BASE_PORT + 4;
    start_server_noauth(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("127.0.0.1", port, true, false, true, None, None).await;
    assert!(_tx > 0, "UDP4 send: expected tx > 0, got {}", _tx);
}

#[tokio::test]
async fn test_udp4_both() {
    let port = BASE_PORT + 5;
    start_server_noauth(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("127.0.0.1", port, true, true, true, None, None).await;
    assert!(_tx > 0, "UDP4 both: expected tx > 0, got {}", _tx);
    assert!(_rx > 0, "UDP4 both: expected rx > 0, got {}", _rx);
}

// --- TCP IPv6 Tests ---

#[tokio::test]
async fn test_tcp6_receive() {
    let port = BASE_PORT + 6;
    start_server_v6(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("::1", port, false, true, false, None, None).await;
    assert!(_rx > 0, "TCP6 receive: expected rx > 0, got {}", _rx);
}

#[tokio::test]
async fn test_tcp6_send() {
    let port = BASE_PORT + 7;
    start_server_v6(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("::1", port, true, false, false, None, None).await;
    assert!(_tx > 0, "TCP6 send: expected tx > 0, got {}", _tx);
}

#[tokio::test]
async fn test_tcp6_both() {
    let port = BASE_PORT + 8;
    start_server_v6(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("::1", port, true, true, false, None, None).await;
    assert!(_tx > 0, "TCP6 both: expected tx > 0, got {}", _tx);
    assert!(_rx > 0, "TCP6 both: expected rx > 0, got {}", _rx);
}

// --- UDP IPv6 Tests (loopback, no ENOBUFS issues) ---

#[tokio::test]
async fn test_udp6_receive() {
    let port = BASE_PORT + 9;
    start_server_v6(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("::1", port, false, true, true, None, None).await;
    assert!(_rx > 0, "UDP6 receive: expected rx > 0, got {}", _rx);
}

#[tokio::test]
async fn test_udp6_send() {
    let port = BASE_PORT + 10;
    start_server_v6(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("::1", port, true, false, true, None, None).await;
    assert!(_tx > 0, "UDP6 send: expected tx > 0, got {}", _tx);
}

#[tokio::test]
async fn test_udp6_both() {
    let port = BASE_PORT + 11;
    start_server_v6(port).await;
    let (_tx, _rx, _, _intervals) = run_client_test("::1", port, true, true, true, None, None).await;
    assert!(_tx > 0, "UDP6 both: expected tx > 0, got {}", _tx);
    assert!(_rx > 0, "UDP6 both: expected rx > 0, got {}", _rx);
}

// --- Authentication Tests ---

#[tokio::test]
async fn test_md5_auth_works() {
    let port = BASE_PORT + 12;
    start_server(port, false).await;
    let (_tx, _rx, _, _) = run_client_test(
        "127.0.0.1", port, false, true, false,
        Some("testuser"), Some("testpass"),
    ).await;
    assert!(_rx > 0, "MD5 auth: expected data flow");
}

#[tokio::test]
async fn test_ecsrp5_auth_works() {
    let port = BASE_PORT + 13;
    start_server(port, true).await;
    let (_tx, _rx, _, _) = run_client_test(
        "127.0.0.1", port, false, true, false,
        Some("testuser"), Some("testpass"),
    ).await;
    assert!(_rx > 0, "EC-SRP5 auth: expected data flow");
}

#[tokio::test]
async fn test_ecsrp5_wrong_password() {
    let port = BASE_PORT + 14;
    start_server(port, true).await;
    let state = btest_rs::bandwidth::BandwidthState::new();
    let result = btest_rs::client::run_client(
        "127.0.0.1", port,
        btest_rs::protocol::CMD_DIR_TX,
        false, 0, 0,
        Some("testuser".into()), Some("wrongpass".into()),
        false, state,
    ).await;
    assert!(result.is_err(), "Wrong password should fail");
}

// --- CSV Output Tests ---

#[tokio::test]
async fn test_csv_created_client() {
    let port = BASE_PORT + 15;
    start_server_noauth(port).await;

    let csv_path = format!("/tmp/btest_test_csv_{}.csv", port);
    let _ = std::fs::remove_file(&csv_path);

    // Initialize CSV
    btest_rs::csv_output::init(&csv_path).unwrap();

    let (tx, rx, lost, intervals) = run_client_test(
        "127.0.0.1", port, false, true, false, None, None,
    ).await;

    // Write result like main.rs does
    btest_rs::csv_output::write_result(
        "127.0.0.1", port, "TCP", "receive",
        2, tx, rx, lost, 0, 0, "none",
    );

    // Verify CSV exists and has data
    let content = std::fs::read_to_string(&csv_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert!(lines.len() >= 2, "CSV should have header + at least 1 row, got {} lines", lines.len());
    assert!(lines[0].starts_with("timestamp,"), "CSV header missing");
    assert!(lines[1].contains("TCP"), "CSV row should contain protocol");
    // Check that tx or rx bytes are non-zero (the 7th or 8th CSV field)
    let fields: Vec<&str> = lines[1].split(',').collect();
    assert!(fields.len() >= 10, "CSV row should have enough fields");
    let tx_bytes: u64 = fields[8].parse().unwrap_or(0);
    let rx_bytes: u64 = fields[9].parse().unwrap_or(0);
    assert!(tx_bytes > 0 || rx_bytes > 0, "CSV should have non-zero bytes: tx={} rx={}", tx_bytes, rx_bytes);

    let _ = std::fs::remove_file(&csv_path);
}

#[tokio::test]
async fn test_csv_created_server() {
    let port = BASE_PORT + 16;
    let csv_path = format!("/tmp/btest_test_server_csv_{}.csv", port);
    let _ = std::fs::remove_file(&csv_path);

    btest_rs::csv_output::init(&csv_path).unwrap();
    start_server_noauth(port).await;

    let _ = run_client_test("127.0.0.1", port, false, true, false, None, None).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let content = std::fs::read_to_string(&csv_path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    assert!(lines.len() >= 2, "Server CSV should have header + rows, got {}", lines.len());

    let _ = std::fs::remove_file(&csv_path);
}

// --- Syslog Tests ---

#[tokio::test]
async fn test_syslog_emits_events() {
    // Bind a local UDP socket to receive syslog messages
    let syslog_sock = StdUdpSocket::bind("127.0.0.1:0").unwrap();
    let syslog_addr = syslog_sock.local_addr().unwrap();
    syslog_sock.set_nonblocking(true).unwrap();

    // Initialize syslog to our test socket
    btest_rs::syslog_logger::init(&syslog_addr.to_string()).unwrap();

    let port = BASE_PORT + 17;
    start_server_noauth(port).await;

    let _ = run_client_test("127.0.0.1", port, false, true, false, None, None).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Read all syslog messages
    let mut messages = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match syslog_sock.recv(&mut buf) {
            Ok(n) => messages.push(String::from_utf8_lossy(&buf[..n]).to_string()),
            Err(_) => break,
        }
    }

    let all = messages.join("\n");
    assert!(all.contains("AUTH_SUCCESS") || all.contains("TEST_START"),
        "Syslog should contain auth or test events, got: {}", all);
    assert!(all.contains("TEST_START"), "Syslog should contain TEST_START");
    assert!(all.contains("TEST_END"), "Syslog should contain TEST_END");
}

// --- Bandwidth State Tests ---

#[tokio::test]
async fn test_bandwidth_state_record_interval() {
    let state = btest_rs::bandwidth::BandwidthState::new();
    state.record_interval(1000, 2000, 5);
    state.record_interval(3000, 4000, 10);
    let (tx, rx, lost, intervals) = state.summary();
    assert_eq!(tx, 4000);
    assert_eq!(rx, 6000);
    assert_eq!(lost, 15);
    assert_eq!(intervals, 2);
}

#[tokio::test]
async fn test_bandwidth_state_running_flag() {
    let state = btest_rs::bandwidth::BandwidthState::new();
    assert!(state.running.load(Ordering::Relaxed));
    state.running.store(false, Ordering::SeqCst);
    assert!(!state.running.load(Ordering::Relaxed));
}
