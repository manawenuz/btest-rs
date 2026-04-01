//! Lightweight CPU usage measurement.
//!
//! Returns the system-wide CPU usage as a percentage (0-100).
//! Works on macOS and Linux without external dependencies.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

static CURRENT_CPU: AtomicU8 = AtomicU8::new(0);

/// Start a background thread that samples CPU usage every second.
pub fn start_sampler() {
    std::thread::spawn(|| {
        let mut prev = get_cpu_times();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let curr = get_cpu_times();
            let usage = compute_usage(&prev, &curr);
            CURRENT_CPU.store(usage, Ordering::Relaxed);
            prev = curr;
        }
    });
}

/// Get the current CPU usage percentage (0-100).
pub fn get() -> u8 {
    CURRENT_CPU.load(Ordering::Relaxed)
}

// --- Platform-specific implementation ---

#[cfg(target_os = "linux")]
fn get_cpu_times() -> (u64, u64) {
    // Read /proc/stat: cpu  user nice system idle iowait irq softirq steal
    if let Ok(content) = std::fs::read_to_string("/proc/stat") {
        if let Some(line) = content.lines().next() {
            let parts: Vec<u64> = line
                .split_whitespace()
                .skip(1) // skip "cpu"
                .filter_map(|s| s.parse().ok())
                .collect();
            if parts.len() >= 4 {
                let idle = parts[3];
                let total: u64 = parts.iter().sum();
                return (total, idle);
            }
        }
    }
    (0, 0)
}

#[cfg(target_os = "macos")]
fn get_cpu_times() -> (u64, u64) {
    // Use host_statistics to get CPU ticks
    use std::mem::MaybeUninit;

    extern "C" {
        fn mach_host_self() -> u32;
        fn host_statistics(
            host: u32,
            flavor: i32,
            info: *mut i32,
            count: *mut u32,
        ) -> i32;
    }

    const HOST_CPU_LOAD_INFO: i32 = 3;
    const CPU_STATE_MAX: usize = 4;

    unsafe {
        let host = mach_host_self();
        let mut info = MaybeUninit::<[u32; CPU_STATE_MAX]>::uninit();
        let mut count: u32 = CPU_STATE_MAX as u32;

        let ret = host_statistics(
            host,
            HOST_CPU_LOAD_INFO,
            info.as_mut_ptr() as *mut i32,
            &mut count,
        );

        if ret == 0 {
            let ticks = info.assume_init();
            // ticks: [user, system, idle, nice]
            let user = ticks[0] as u64;
            let system = ticks[1] as u64;
            let idle = ticks[2] as u64;
            let nice = ticks[3] as u64;
            let total = user + system + idle + nice;
            return (total, idle);
        }
    }
    (0, 0)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn get_cpu_times() -> (u64, u64) {
    (0, 0) // Unsupported platform
}

fn compute_usage(prev: &(u64, u64), curr: &(u64, u64)) -> u8 {
    let total_diff = curr.0.saturating_sub(prev.0);
    let idle_diff = curr.1.saturating_sub(prev.1);
    if total_diff == 0 {
        return 0;
    }
    let busy = total_diff - idle_diff;
    ((busy * 100) / total_diff).min(100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_times_returns_nonzero() {
        let (total, idle) = get_cpu_times();
        // On supported platforms, total should be > 0
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(total > 0, "CPU total ticks should be > 0");
            assert!(idle <= total, "idle should be <= total");
        }
    }

    #[test]
    fn test_compute_usage() {
        assert_eq!(compute_usage(&(0, 0), &(100, 20)), 80);
        assert_eq!(compute_usage(&(0, 0), &(100, 100)), 0);
        assert_eq!(compute_usage(&(0, 0), &(100, 0)), 100);
        assert_eq!(compute_usage(&(0, 0), &(0, 0)), 0);
    }
}
