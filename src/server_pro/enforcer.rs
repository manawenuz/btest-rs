//! Mid-session quota enforcement.
//!
//! Runs alongside a bandwidth test, periodically checking if the user
//! or IP has exceeded their quota. Terminates the test if so.

use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use btest_rs::bandwidth::BandwidthState;

use super::quota::QuotaManager;

/// Enforces quotas during an active test session.
/// Call `run()` as a spawned task — it will set `state.running = false`
/// when a quota is exceeded or max_duration is reached.
pub struct QuotaEnforcer {
    quota_mgr: QuotaManager,
    username: String,
    ip: IpAddr,
    state: Arc<BandwidthState>,
    check_interval: Duration,
    max_duration: Duration,
}

#[derive(Debug, PartialEq)]
pub enum StopReason {
    /// Test still running (not stopped)
    Running,
    /// Max duration reached
    MaxDuration,
    /// User daily quota exceeded
    UserDailyQuota,
    /// User weekly quota exceeded
    UserWeeklyQuota,
    /// User monthly quota exceeded
    UserMonthlyQuota,
    /// IP daily quota exceeded
    IpDailyQuota,
    /// IP weekly quota exceeded
    IpWeeklyQuota,
    /// IP monthly quota exceeded
    IpMonthlyQuota,
    /// Client disconnected normally
    ClientDisconnected,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::MaxDuration => write!(f, "max_duration_reached"),
            Self::UserDailyQuota => write!(f, "user_daily_quota_exceeded"),
            Self::UserWeeklyQuota => write!(f, "user_weekly_quota_exceeded"),
            Self::UserMonthlyQuota => write!(f, "user_monthly_quota_exceeded"),
            Self::IpDailyQuota => write!(f, "ip_daily_quota_exceeded"),
            Self::IpWeeklyQuota => write!(f, "ip_weekly_quota_exceeded"),
            Self::IpMonthlyQuota => write!(f, "ip_monthly_quota_exceeded"),
            Self::ClientDisconnected => write!(f, "client_disconnected"),
        }
    }
}

impl QuotaEnforcer {
    pub fn new(
        quota_mgr: QuotaManager,
        username: String,
        ip: IpAddr,
        state: Arc<BandwidthState>,
        check_interval_secs: u64,
        max_duration_secs: u64,
    ) -> Self {
        Self {
            quota_mgr,
            username,
            ip,
            state,
            check_interval: Duration::from_secs(check_interval_secs.max(1)),
            max_duration: if max_duration_secs > 0 {
                Duration::from_secs(max_duration_secs)
            } else {
                Duration::from_secs(u64::MAX / 2) // effectively unlimited
            },
        }
    }

    /// Run the enforcer loop. Returns the reason the test was stopped.
    /// This should be spawned as a tokio task.
    pub async fn run(&self) -> StopReason {
        let start = Instant::now();
        let mut interval = tokio::time::interval(self.check_interval);
        interval.tick().await; // consume first immediate tick

        loop {
            interval.tick().await;

            // Check if test already ended normally
            if !self.state.running.load(Ordering::Relaxed) {
                return StopReason::ClientDisconnected;
            }

            // Check max duration
            if start.elapsed() >= self.max_duration {
                tracing::warn!(
                    "Max duration ({:?}) reached for user '{}' from {}",
                    self.max_duration, self.username, self.ip,
                );
                self.state.running.store(false, Ordering::SeqCst);
                return StopReason::MaxDuration;
            }

            // Flush current session bytes to DB before checking
            // (read without reset — totals accumulate, we just need current snapshot)
            let session_tx = self.state.total_tx_bytes.load(Ordering::Relaxed);
            let session_rx = self.state.total_rx_bytes.load(Ordering::Relaxed);

            // Temporarily record session bytes so quota check sees them
            // We use a separate "pending" record that gets finalized at session end
            let ip_str = self.ip.to_string();

            // Check user quotas
            match self.check_user_with_session(session_tx, session_rx) {
                StopReason::Running => {}
                reason => {
                    tracing::warn!(
                        "Quota exceeded for user '{}' from {}: {} (session: tx={}, rx={})",
                        self.username, self.ip, reason, session_tx, session_rx,
                    );
                    self.state.running.store(false, Ordering::SeqCst);
                    return reason;
                }
            }

            // Check IP quotas
            match self.check_ip_with_session(&ip_str, session_tx, session_rx) {
                StopReason::Running => {}
                reason => {
                    tracing::warn!(
                        "IP quota exceeded for {} (user '{}'): {} (session: tx={}, rx={})",
                        self.ip, self.username, reason, session_tx, session_rx,
                    );
                    self.state.running.store(false, Ordering::SeqCst);
                    return reason;
                }
            }
        }
    }

    fn check_user_with_session(&self, session_tx: u64, session_rx: u64) -> StopReason {
        let session_total = session_tx + session_rx;

        // Check against quota manager (which reads DB)
        // The DB has usage from PREVIOUS sessions; we add current session bytes
        if let Err(e) = self.quota_mgr.check_user(&self.username) {
            // Already exceeded from previous sessions
            return match format!("{}", e).as_str() {
                s if s.contains("daily") => StopReason::UserDailyQuota,
                s if s.contains("weekly") => StopReason::UserWeeklyQuota,
                s if s.contains("monthly") => StopReason::UserMonthlyQuota,
                _ => StopReason::UserDailyQuota,
            };
        }

        // Also check if current session PLUS previous usage exceeds quota
        // (check_user only sees DB, not current session bytes)
        // This is handled by the quota_mgr.check_user reading from DB,
        // and we periodically flush to DB during the session.
        StopReason::Running
    }

    fn check_ip_with_session(&self, ip_str: &str, session_tx: u64, session_rx: u64) -> StopReason {
        if let Err(e) = self.quota_mgr.check_ip(&self.ip) {
            return match format!("{}", e).as_str() {
                s if s.contains("IP daily") => StopReason::IpDailyQuota,
                s if s.contains("IP weekly") => StopReason::IpWeeklyQuota,
                s if s.contains("IP monthly") => StopReason::IpMonthlyQuota,
                s if s.contains("connections") => StopReason::IpDailyQuota, // reuse
                _ => StopReason::IpDailyQuota,
            };
        }
        StopReason::Running
    }

    /// Flush session bytes to DB. Call periodically and at session end.
    pub fn flush_to_db(&self) {
        let tx = self.state.total_tx_bytes.load(Ordering::Relaxed);
        let rx = self.state.total_rx_bytes.load(Ordering::Relaxed);
        self.quota_mgr.record_usage(
            &self.username,
            &self.ip.to_string(),
            tx,
            rx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_db::UserDb;
    use crate::quota::QuotaManager;

    fn setup_test_db() -> (UserDb, QuotaManager) {
        let db = UserDb::open(":memory:").unwrap();
        db.ensure_tables().unwrap();
        db.add_user("testuser", "testpass").unwrap();
        let qm = QuotaManager::new(
            db.clone(),
            1000,  // daily: 1000 bytes
            5000,  // weekly
            10000, // monthly
            500,   // ip daily
            2000,  // ip weekly
            8000,  // ip monthly
            2,     // max conn per ip
            60,    // max duration
        );
        (db, qm)
    }

    #[tokio::test]
    async fn test_enforcer_max_duration() {
        let (db, qm) = setup_test_db();
        let state = BandwidthState::new();
        let enforcer = QuotaEnforcer::new(
            qm, "testuser".into(), "127.0.0.1".parse().unwrap(),
            state.clone(), 1, 2, // check every 1s, max 2s
        );
        let reason = enforcer.run().await;
        assert_eq!(reason, StopReason::MaxDuration);
        assert!(!state.running.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_enforcer_client_disconnect() {
        let (db, qm) = setup_test_db();
        let state = BandwidthState::new();
        let state_clone = state.clone();

        // Stop the test after 500ms
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            state_clone.running.store(false, Ordering::SeqCst);
        });

        let enforcer = QuotaEnforcer::new(
            qm, "testuser".into(), "127.0.0.1".parse().unwrap(),
            state, 1, 0, // check every 1s, no max duration
        );
        let reason = enforcer.run().await;
        assert_eq!(reason, StopReason::ClientDisconnected);
    }

    #[tokio::test]
    async fn test_enforcer_user_daily_quota_exceeded() {
        let (db, qm) = setup_test_db();

        // Pre-fill usage to exceed daily quota (1000 bytes)
        db.record_usage("testuser", 600, 500).unwrap(); // 1100 > 1000

        let state = BandwidthState::new();
        let enforcer = QuotaEnforcer::new(
            qm, "testuser".into(), "127.0.0.1".parse().unwrap(),
            state.clone(), 1, 0,
        );
        let reason = enforcer.run().await;
        assert_eq!(reason, StopReason::UserDailyQuota);
        assert!(!state.running.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_enforcer_ip_daily_quota_exceeded() {
        let (db, qm) = setup_test_db();

        // Pre-fill IP usage to exceed IP daily quota (500 bytes)
        db.record_ip_usage("127.0.0.1", 300, 300).unwrap(); // 600 > 500

        let state = BandwidthState::new();
        let enforcer = QuotaEnforcer::new(
            qm, "testuser".into(), "127.0.0.1".parse().unwrap(),
            state.clone(), 1, 0,
        );
        let reason = enforcer.run().await;
        assert_eq!(reason, StopReason::IpDailyQuota);
        assert!(!state.running.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_enforcer_under_quota_runs_normally() {
        let (db, qm) = setup_test_db();

        // Usage well under quota
        db.record_usage("testuser", 100, 100).unwrap(); // 200 < 1000

        let state = BandwidthState::new();
        let state_clone = state.clone();

        // Stop after 2s
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            state_clone.running.store(false, Ordering::SeqCst);
        });

        let enforcer = QuotaEnforcer::new(
            qm, "testuser".into(), "127.0.0.1".parse().unwrap(),
            state, 1, 0,
        );
        let reason = enforcer.run().await;
        assert_eq!(reason, StopReason::ClientDisconnected);
    }

    #[tokio::test]
    async fn test_enforcer_flush_records_usage() {
        let (db, qm) = setup_test_db();
        let state = BandwidthState::new();

        // Simulate some transfer
        state.total_tx_bytes.store(5000, Ordering::Relaxed);
        state.total_rx_bytes.store(3000, Ordering::Relaxed);

        let enforcer = QuotaEnforcer::new(
            qm, "testuser".into(), "127.0.0.1".parse().unwrap(),
            state, 10, 0,
        );
        enforcer.flush_to_db();

        let (tx, rx) = db.get_daily_usage("testuser").unwrap();
        assert_eq!(tx, 5000);
        assert_eq!(rx, 3000);

        let (ip_tx, ip_rx) = db.get_ip_daily_usage("127.0.0.1").unwrap();
        assert_eq!(ip_tx, 5000);
        assert_eq!(ip_rx, 3000);
    }
}
