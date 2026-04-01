//! CSV output for machine-readable test results.
//!
//! Appends a row per test to the specified CSV file.
//! Creates the file with headers if it doesn't exist.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

static CSV_FILE: Mutex<Option<String>> = Mutex::new(None);
static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const HEADER: &str = "timestamp,host,port,protocol,direction,duration_s,tx_avg_mbps,rx_avg_mbps,tx_bytes,rx_bytes,lost_packets,local_cpu_pct,remote_cpu_pct,auth_type";

/// Initialize CSV output. Creates file with headers if needed.
pub fn init(path: &str) -> std::io::Result<()> {
    let needs_header = !Path::new(path).exists() || std::fs::metadata(path)?.len() == 0;

    if needs_header {
        let mut f = OpenOptions::new().create(true).write(true).open(path)?;
        writeln!(f, "{}", HEADER)?;
    }

    *CSV_FILE.lock().unwrap() = Some(path.to_string());
    Ok(())
}

pub fn set_quiet(q: bool) {
    QUIET.store(q, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_quiet() -> bool {
    QUIET.load(std::sync::atomic::Ordering::Relaxed)
}

/// Write a test result row to the CSV file.
pub fn write_result(
    host: &str,
    port: u16,
    protocol: &str,
    direction: &str,
    duration_secs: u64,
    tx_bytes: u64,
    rx_bytes: u64,
    lost_packets: u64,
    local_cpu: u8,
    remote_cpu: u8,
    auth_type: &str,
) {
    let guard = CSV_FILE.lock().unwrap();
    if let Some(ref path) = *guard {
        let tx_mbps = if duration_secs > 0 {
            tx_bytes as f64 * 8.0 / duration_secs as f64 / 1_000_000.0
        } else {
            0.0
        };
        let rx_mbps = if duration_secs > 0 {
            rx_bytes as f64 * 8.0 / duration_secs as f64 / 1_000_000.0
        } else {
            0.0
        };

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let row = format!(
            "{},{},{},{},{},{},{:.2},{:.2},{},{},{},{},{},{}",
            now, host, port, protocol, direction, duration_secs,
            tx_mbps, rx_mbps, tx_bytes, rx_bytes, lost_packets,
            local_cpu, remote_cpu, auth_type,
        );

        if let Ok(mut f) = OpenOptions::new().append(true).open(path) {
            let _ = writeln!(f, "{}", row);
        }
    }
}

/// Check if CSV output is enabled.
pub fn is_enabled() -> bool {
    CSV_FILE.lock().unwrap().is_some()
}
