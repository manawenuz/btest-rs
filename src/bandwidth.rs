use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Session-wide asynchronous rate limiter.
///
/// Pacing every UDP datagram with `tokio::time::sleep()` severely undershoots
/// on platforms with coarse timers (notably Windows).  Instead, this limiter
/// schedules bytes against one shared timeline and sleeps only after at least
/// a small quantum has accumulated.  Oversleep is recovered by a bounded
/// catch-up burst, while sharing the limiter across TCP connections keeps the
/// configured speed as a session total rather than a per-connection limit.
#[derive(Debug)]
pub struct RateLimiter {
    rate_bps: AtomicU32,
    next_send: tokio::sync::Mutex<Instant>,
}

impl RateLimiter {
    const MIN_SLEEP: Duration = Duration::from_millis(2);
    const MAX_CATCH_UP: Duration = Duration::from_millis(50);

    pub fn new(rate_bps: u32) -> Arc<Self> {
        Arc::new(Self {
            rate_bps: AtomicU32::new(rate_bps),
            next_send: tokio::sync::Mutex::new(Instant::now()),
        })
    }

    /// Reset the schedule when the peer changes the requested rate.
    pub async fn set_rate(&self, rate_bps: u32) {
        self.rate_bps.store(rate_bps, std::sync::atomic::Ordering::Relaxed);
        *self.next_send.lock().await = Instant::now();
    }

    /// Account for bytes just sent and wait until their scheduled send time.
    pub async fn pace(&self, bytes: usize) {
        let rate_bps = self.rate_bps.load(std::sync::atomic::Ordering::Relaxed);
        if rate_bps == 0 || bytes == 0 {
            return;
        }

        let nanos = ((bytes as u128 * 8 * 1_000_000_000u128) / rate_bps as u128)
            .max(1)
            .min(u64::MAX as u128) as u64;
        let interval = Duration::from_nanos(nanos);
        let now = Instant::now();

        let delay = {
            let mut next_send = self.next_send.lock().await;
            if next_send.checked_add(Self::MAX_CATCH_UP).is_some_and(|limit| limit < now) {
                *next_send = now;
            }
            *next_send += interval;
            next_send.saturating_duration_since(now)
        };

        // Accumulate sub-millisecond packet intervals into a small batch.
        // This avoids depending on the operating system's timer resolution.
        if delay >= Self::MIN_SLEEP {
            tokio::time::sleep(delay).await;
        }
    }
}

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
    #[cfg(feature = "pro")]
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

    #[tokio::test]
    async fn shared_rate_limiter_uses_one_timeline() {
        let limiter = RateLimiter::new(8_000_000);
        let started = Instant::now();
        let mut workers = Vec::new();
        for _ in 0..2 {
            let limiter = limiter.clone();
            workers.push(tokio::spawn(async move {
                for _ in 0..50 {
                    limiter.pace(1_000).await;
                }
            }));
        }
        for worker in workers {
            worker.await.unwrap();
        }

        // 100,000 bytes at 8 Mbps takes 100 ms. Allow scheduler jitter while
        // still proving that two workers do not each receive the full rate.
        assert!(started.elapsed() >= Duration::from_millis(80));
    }

    #[test]
    fn test_format_bandwidth() {
        assert_eq!(format_bandwidth(100_000_000.0), "100.00 Mbps");
        assert_eq!(format_bandwidth(1_500_000_000.0), "1.50 Gbps");
        assert_eq!(format_bandwidth(500_000.0), "500.00 Kbps");
    }
}
