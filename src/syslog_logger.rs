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
        // RFC 3164 (BSD syslog): <priority>Mon DD HH:MM:SS hostname program: message
        // facility=16 (local0) * 8 + severity
        let priority = 128 + severity;
        let timestamp = bsd_timestamp();
        let syslog_msg = format!(
            "<{}>{} {} btest-rs: {}",
            priority, timestamp, sender.hostname, msg,
        );
        let _ = sender.socket.send_to(syslog_msg.as_bytes(), &sender.target);
    }
}

fn bsd_timestamp() -> String {
    // RFC 3164 format: "Mon DD HH:MM:SS" (no year)
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Simple conversion — good enough for syslog
    let secs_in_day = 86400u64;
    let days = now / secs_in_day;
    let time_of_day = now % secs_in_day;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Day of year calculation (approximate months)
    let months = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
    let days_in_months = [31u64,28,31,30,31,30,31,31,30,31,30,31];

    // Days since epoch to year/month/day
    let mut y = 1970u64;
    let mut remaining = days;
    loop {
        let leap = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < leap { break; }
        remaining -= leap;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mut m = 0usize;
    for i in 0..12 {
        let mut d = days_in_months[i];
        if i == 1 && leap { d += 1; }
        if remaining < d { m = i; break; }
        remaining -= d;
    }
    let day = remaining + 1;

    format!("{} {:2} {:02}:{:02}:{:02}", months[m], day, hours, minutes, seconds)
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

pub fn test_end(
    peer: &str,
    proto: &str,
    direction: &str,
    total_tx: u64,
    total_rx: u64,
    total_lost: u64,
    duration_secs: u32,
) {
    let tx_mbps = if duration_secs > 0 {
        total_tx as f64 * 8.0 / duration_secs as f64 / 1_000_000.0
    } else {
        0.0
    };
    let rx_mbps = if duration_secs > 0 {
        total_rx as f64 * 8.0 / duration_secs as f64 / 1_000_000.0
    } else {
        0.0
    };
    let msg = format!(
        "TEST_END peer={} proto={} dir={} duration={}s tx_avg={:.2}Mbps rx_avg={:.2}Mbps tx_bytes={} rx_bytes={} lost={}",
        peer, proto, direction, duration_secs, tx_mbps, rx_mbps, total_tx, total_rx, total_lost,
    );
    tracing::info!("{}", msg);
    send(6, &msg);
}

/// Check if syslog is enabled.
pub fn is_enabled() -> bool {
    SYSLOG.lock().unwrap().is_some()
}
