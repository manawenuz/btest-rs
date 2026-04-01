use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Duration;

/// Shared state for bandwidth tracking between TX/RX threads and status reporter.
#[derive(Debug)]
pub struct BandwidthState {
    pub tx_bytes: AtomicU64,
    pub rx_bytes: AtomicU64,
    pub tx_speed: AtomicU32,
    pub tx_speed_changed: AtomicBool,
    pub running: AtomicBool,
    pub rx_packets: AtomicU64,
    pub rx_lost_packets: AtomicU64,
    pub last_udp_seq: AtomicU32,
    /// Cumulative totals (never reset by swap)
    pub total_tx_bytes: AtomicU64,
    pub total_rx_bytes: AtomicU64,
    pub total_lost_packets: AtomicU64,
    pub intervals: AtomicU32,
    /// Remote peer's CPU usage (received via status messages)
    pub remote_cpu: AtomicU8,
    /// Remaining byte budget (TX + RX combined). When this reaches 0 the test
    /// stops immediately. u64::MAX means unlimited (default for non-pro server).
    pub byte_budget: AtomicU64,
}

impl BandwidthState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tx_bytes: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            tx_speed: AtomicU32::new(0),
            tx_speed_changed: AtomicBool::new(false),
            running: AtomicBool::new(true),
            rx_packets: AtomicU64::new(0),
            rx_lost_packets: AtomicU64::new(0),
            last_udp_seq: AtomicU32::new(0),
            total_tx_bytes: AtomicU64::new(0),
            total_rx_bytes: AtomicU64::new(0),
            total_lost_packets: AtomicU64::new(0),
            intervals: AtomicU32::new(0),
            remote_cpu: AtomicU8::new(0),
            byte_budget: AtomicU64::new(u64::MAX),
        })
    }

    /// Record an interval's stats into cumulative totals.
    pub fn record_interval(&self, tx: u64, rx: u64, lost: u64) {
        use std::sync::atomic::Ordering::Relaxed;
        self.total_tx_bytes.fetch_add(tx, Relaxed);
        self.total_rx_bytes.fetch_add(rx, Relaxed);
        self.total_lost_packets.fetch_add(lost, Relaxed);
        self.intervals.fetch_add(1, Relaxed);
    }

    /// Try to spend `amount` bytes from the budget. Returns `true` if allowed,
    /// `false` if the budget is exhausted (and sets `running = false`).
    #[inline]
    pub fn spend_budget(&self, amount: u64) -> bool {
        use std::sync::atomic::Ordering::{Relaxed, SeqCst};
        // Fast path: unlimited budget (non-pro server)
        let current = self.byte_budget.load(Relaxed);
        if current == u64::MAX {
            return true;
        }
        if current < amount {
            self.running.store(false, SeqCst);
            return false;
        }
        self.byte_budget.fetch_sub(amount, Relaxed);
        true
    }

    /// Set the byte budget (total bytes allowed for the entire test).
    pub fn set_budget(&self, budget: u64) {
        self.byte_budget.store(budget, std::sync::atomic::Ordering::SeqCst);
    }

    /// Get summary for syslog reporting.
    pub fn summary(&self) -> (u64, u64, u64, u32) {
        use std::sync::atomic::Ordering::Relaxed;
        (
            self.total_tx_bytes.load(Relaxed),
            self.total_rx_bytes.load(Relaxed),
            self.total_lost_packets.load(Relaxed),
            self.intervals.load(Relaxed),
        )
    }
}

/// Calculate the sleep interval between packets to achieve target bandwidth.
/// Returns None if speed is unlimited (0).
pub fn calc_send_interval(tx_speed_bps: u32, tx_size: u16) -> Option<Duration> {
    if tx_speed_bps == 0 {
        return None;
    }

    let bits_per_packet = tx_size as u64 * 8;
    let interval_ns = (1_000_000_000u64 * bits_per_packet) / tx_speed_bps as u64;

    // Replicate MikroTik behavior: if interval > 500ms, clamp to 1 second
    if interval_ns > 500_000_000 {
        Some(Duration::from_secs(1))
    } else {
        Some(Duration::from_nanos(interval_ns.max(1)))
    }
}

/// Advance `next_send` by one interval and clamp drift.
///
/// When the sender falls behind (e.g., the write blocked longer than the
/// inter-packet interval), `next_send` accumulates a debt.  Once the path
/// clears, the loop would fire packets with *no* delay until the debt is
/// repaid, producing a burst that overshoots the target rate.
///
/// This helper resets `next_send` to `now` whenever it has drifted more
/// than 2x the interval behind the current wall-clock time, bounding the
/// maximum burst to at most one extra interval's worth of packets.
pub fn advance_next_send(
    next_send: &mut std::time::Instant,
    iv: Duration,
    now: std::time::Instant,
) -> Option<Duration> {
    *next_send += iv;
    // If we have fallen more than 2x the interval behind, reset to now
    // to prevent a compensating burst.
    if *next_send + iv < now {
        *next_send = now;
    }
    if *next_send > now {
        Some(*next_send - now)
    } else {
        None
    }
}

/// Format a bandwidth value in human-readable form.
pub fn format_bandwidth(bits_per_sec: f64) -> String {
    if bits_per_sec >= 1_000_000_000.0 {
        format!("{:.2} Gbps", bits_per_sec / 1_000_000_000.0)
    } else if bits_per_sec >= 1_000_000.0 {
        format!("{:.2} Mbps", bits_per_sec / 1_000_000.0)
    } else if bits_per_sec >= 1_000.0 {
        format!("{:.2} Kbps", bits_per_sec / 1_000.0)
    } else {
        format!("{:.0} bps", bits_per_sec)
    }
}

/// Parse bandwidth string like "100M", "1G", "500K", "1000000"
pub fn parse_bandwidth(s: &str) -> std::result::Result<u32, anyhow::Error> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow::anyhow!("Empty bandwidth string"));
    }

    let (num_str, multiplier) = match s.as_bytes().last() {
        Some(b'G' | b'g') => (&s[..s.len() - 1], 1_000_000_000u64),
        Some(b'M' | b'm') => (&s[..s.len() - 1], 1_000_000u64),
        Some(b'K' | b'k') => (&s[..s.len() - 1], 1_000u64),
        _ => (s, 1u64),
    };

    let num: f64 = num_str
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid bandwidth number '{}': {}", num_str, e))?;
    let result = (num * multiplier as f64) as u64;
    if result > u32::MAX as u64 {
        Err(anyhow::anyhow!("Bandwidth {} exceeds maximum (4 Gbps)", s))
    } else {
        Ok(result as u32)
    }
}

/// Print a status line for a reporting interval.
pub fn print_status(
    interval_num: u32,
    direction: &str,
    bytes: u64,
    elapsed: Duration,
    lost_packets: Option<u64>,
) {
    print_status_with_cpu(interval_num, direction, bytes, elapsed, lost_packets, None, None);
}

pub fn print_status_with_cpu(
    interval_num: u32,
    direction: &str,
    bytes: u64,
    elapsed: Duration,
    lost_packets: Option<u64>,
    local_cpu: Option<u8>,
    remote_cpu: Option<u8>,
) {
    if crate::csv_output::is_quiet() {
        return;
    }

    let secs = elapsed.as_secs_f64();
    let bits = bytes as f64 * 8.0;
    let bw = if secs > 0.0 { bits / secs } else { 0.0 };

    let loss_str = match lost_packets {
        Some(lost) if lost > 0 => format!("  lost: {}", lost),
        _ => String::new(),
    };

    let cpu_str = match (local_cpu, remote_cpu) {
        (Some(l), Some(r)) => {
            let warn = if l > 70 || r > 70 { " !" } else { "" };
            format!("  cpu: {}%/{}%{}", l, r, warn)
        }
        (Some(l), None) => {
            let warn = if l > 70 { " !" } else { "" };
            format!("  cpu: {}%{}", l, warn)
        }
        _ => String::new(),
    };

    println!(
        "[{:4}] {:>3}  {} ({} bytes){}{}",
        interval_num,
        direction,
        format_bandwidth(bw),
        bytes,
        loss_str,
        cpu_str,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bandwidth() {
        assert_eq!(parse_bandwidth("100M").unwrap(), 100_000_000);
        assert_eq!(parse_bandwidth("1G").unwrap(), 1_000_000_000);
        assert_eq!(parse_bandwidth("500K").unwrap(), 500_000);
        assert_eq!(parse_bandwidth("1000000").unwrap(), 1_000_000);
        assert_eq!(parse_bandwidth("1.5M").unwrap(), 1_500_000);
    }

    #[test]
    fn test_calc_interval() {
        // 100Mbps with 1500 byte packets
        let interval = calc_send_interval(100_000_000, 1500).unwrap();
        // Expected: (1e9 * 1500 * 8) / 100_000_000 = 120_000 ns = 120 us
        assert_eq!(interval.as_nanos(), 120_000);

        // Unlimited
        assert!(calc_send_interval(0, 1500).is_none());
    }

    #[test]
    fn test_format_bandwidth() {
        assert_eq!(format_bandwidth(100_000_000.0), "100.00 Mbps");
        assert_eq!(format_bandwidth(1_500_000_000.0), "1.50 Gbps");
        assert_eq!(format_bandwidth(500_000.0), "500.00 Kbps");
    }
}
