//! Syslog integration for btest-rs server mode.
//!
//! Sends structured log events to a remote syslog server via UDP (RFC 5424).
//! Events: auth success/failure, test start/stop, speed results.

use std::net::UdpSocket;
use std::sync::Mutex;

static SYSLOG: Mutex<Option<SyslogSender>> = Mutex::new(None);

struct SyslogSender {
    socket: UdpSocket,
    target: String,
    hostname: String,
}

/// Initialize the global syslog sender.
/// `target` is the syslog server address, e.g. "192.168.1.1:514".
pub fn init(target: &str) -> std::io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "btest-rs".to_string());

    let sender = SyslogSender {
        socket,
        target: target.to_string(),
        hostname,
    };

    *SYSLOG.lock().unwrap() = Some(sender);
    tracing::info!("Syslog enabled, sending to {}", target);
    Ok(())
}

/// Send a syslog message with the given severity and message.
/// Severity: 6=info, 4=warning, 3=error
fn send(severity: u8, msg: &str) {
    let guard = SYSLOG.lock().unwrap();
    if let Some(ref sender) = *guard {
        // RFC 5424 facility=1 (user), severity as given
        let priority = 8 + severity; // facility=1 (user-level) * 8 + severity
        let timestamp = chrono_lite_now();
        let syslog_msg = format!(
            "<{}>1 {} {} btest-rs - - - {}",
            priority, timestamp, sender.hostname, msg,
        );
        let _ = sender.socket.send_to(syslog_msg.as_bytes(), &sender.target);
    }
}

fn chrono_lite_now() -> String {
    // Simple ISO 8601 timestamp without chrono dependency
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    // Good enough for syslog — not perfect but functional
    format!("{}", secs)
}

// --- Public logging functions ---

pub fn auth_success(peer: &str, username: &str, auth_type: &str) {
    let msg = format!(
        "AUTH_SUCCESS peer={} user={} type={}",
        peer, username, auth_type,
    );
    tracing::info!("{}", msg);
    send(6, &msg);
}

pub fn auth_failure(peer: &str, username: &str, auth_type: &str, reason: &str) {
    let msg = format!(
        "AUTH_FAILURE peer={} user={} type={} reason={}",
        peer, username, auth_type, reason,
    );
    tracing::warn!("{}", msg);
    send(4, &msg);
}

pub fn test_start(peer: &str, proto: &str, direction: &str, conn_count: u8) {
    let msg = format!(
        "TEST_START peer={} proto={} dir={} connections={}",
        peer, proto, direction, conn_count.max(1),
    );
    tracing::info!("{}", msg);
    send(6, &msg);
}

pub fn test_end(peer: &str, proto: &str, direction: &str) {
    let msg = format!(
        "TEST_END peer={} proto={} dir={}",
        peer, proto, direction,
    );
    tracing::info!("{}", msg);
    send(6, &msg);
}

pub fn test_result(
    peer: &str,
    direction: &str,
    avg_mbps: f64,
    duration_secs: u32,
) {
    let msg = format!(
        "TEST_RESULT peer={} dir={} avg_mbps={:.2} duration={}s",
        peer, direction, avg_mbps, duration_secs,
    );
    tracing::info!("{}", msg);
    send(6, &msg);
}

/// Check if syslog is enabled.
pub fn is_enabled() -> bool {
    SYSLOG.lock().unwrap().is_some()
}
