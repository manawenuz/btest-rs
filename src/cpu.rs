//! Lightweight CPU usage measurement.
//!
//! Returns the system-wide CPU usage as a percentage (0-100).
//! Works on macOS, Linux, Windows, and FreeBSD without external dependencies.

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

#[cfg(any(target_os = "linux", target_os = "android"))]
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

#[cfg(target_os = "windows")]
fn get_cpu_times() -> (u64, u64) {
    #[repr(C)]
    #[derive(Default)]
    #[allow(non_snake_case)]
    struct FILETIME {
        dwLowDateTime: u32,
        dwHighDateTime: u32,
    }

    impl FILETIME {
        fn to_u64(&self) -> u64 {
            (self.dwHighDateTime as u64) << 32 | self.dwLowDateTime as u64
        }
    }

    extern "system" {
        fn GetSystemTimes(
            lpIdleTime: *mut FILETIME,
            lpKernelTime: *mut FILETIME,
            lpUserTime: *mut FILETIME,
        ) -> i32;
    }

    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();

    // SAFETY: We pass valid pointers to stack-allocated FILETIME structs.
    // GetSystemTimes is a well-documented Win32 API that writes into these
    // output parameters. A non-zero return value indicates success.
    let ret = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };

    if ret != 0 {
        let idle_ticks = idle.to_u64();
        // Kernel time includes idle time on Windows, so total = kernel + user.
        let total_ticks = kernel.to_u64() + user.to_u64();
        (total_ticks, idle_ticks)
    } else {
        (0, 0)
    }
}

#[cfg(target_os = "freebsd")]
fn get_cpu_times() -> (u64, u64) {
    // kern.cp_time returns: user nice system interrupt idle
    if let Ok(output) = std::process::Command::new("sysctl")
        .arg("-n")
        .arg("kern.cp_time")
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let parts: Vec<u64> = text
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if parts.len() >= 5 {
                let user = parts[0];
                let nice = parts[1];
                let system = parts[2];
                let interrupt = parts[3];
                let idle = parts[4];
                let total = user + nice + system + interrupt + idle;
                return (total, idle);
            }
        }
    }
    (0, 0)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
)))]
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
        if cfg!(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows",
            target_os = "freebsd",
        )) {
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
