//! Bandwidth quota management for btest-server-pro.
//!
//! Enforces per-user and per-IP bandwidth limits (daily/weekly/monthly).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use super::user_db::UserDb;

#[derive(Clone)]
pub struct QuotaManager {
    db: UserDb,
    /// Per-user defaults (0 = unlimited)
    default_daily: u64,
    default_weekly: u64,
    default_monthly: u64,
    /// Per-IP limits (0 = unlimited) — for abuse prevention
    ip_daily: u64,
    ip_weekly: u64,
    ip_monthly: u64,
    /// Max simultaneous connections from one IP
    max_conn_per_ip: u32,
    /// Max test duration in seconds
    max_duration: u64,
    active_connections: Arc<Mutex<HashMap<IpAddr, u32>>>,
}

#[derive(Debug)]
pub enum QuotaError {
    DailyExceeded { used: u64, limit: u64 },
    WeeklyExceeded { used: u64, limit: u64 },
    MonthlyExceeded { used: u64, limit: u64 },
    IpDailyExceeded { used: u64, limit: u64 },
    IpWeeklyExceeded { used: u64, limit: u64 },
    IpMonthlyExceeded { used: u64, limit: u64 },
    TooManyConnections { current: u32, limit: u32 },
    UserDisabled,
    UserNotFound,
}

impl std::fmt::Display for QuotaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DailyExceeded { used, limit } =>
                write!(f, "User daily quota exceeded: {}/{} bytes", used, limit),
            Self::WeeklyExceeded { used, limit } =>
                write!(f, "User weekly quota exceeded: {}/{} bytes", used, limit),
            Self::MonthlyExceeded { used, limit } =>
                write!(f, "User monthly quota exceeded: {}/{} bytes", used, limit),
            Self::IpDailyExceeded { used, limit } =>
                write!(f, "IP daily quota exceeded: {}/{} bytes", used, limit),
            Self::IpWeeklyExceeded { used, limit } =>
                write!(f, "IP weekly quota exceeded: {}/{} bytes", used, limit),
            Self::IpMonthlyExceeded { used, limit } =>
                write!(f, "IP monthly quota exceeded: {}/{} bytes", used, limit),
            Self::TooManyConnections { current, limit } =>
                write!(f, "Too many connections from this IP: {}/{}", current, limit),
            Self::UserDisabled => write!(f, "User account is disabled"),
            Self::UserNotFound => write!(f, "User not found"),
        }
    }
}

impl QuotaManager {
    pub fn new(
        db: UserDb,
        default_daily: u64,
        default_weekly: u64,
        default_monthly: u64,
        ip_daily: u64,
        ip_weekly: u64,
        ip_monthly: u64,
        max_conn_per_ip: u32,
        max_duration: u64,
    ) -> Self {
        Self {
            db,
            default_daily,
            default_weekly,
            default_monthly,
            ip_daily,
            ip_weekly,
            ip_monthly,
            max_conn_per_ip,
            max_duration,
            active_connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if a user is allowed to start a test.
    pub fn check_user(&self, username: &str) -> Result<(), QuotaError> {
        let user = self.db.get_user(username)
            .map_err(|_| QuotaError::UserNotFound)?
            .ok_or(QuotaError::UserNotFound)?;

        if !user.enabled {
            return Err(QuotaError::UserDisabled);
        }

        // Daily
        let daily_limit = if user.daily_quota > 0 { user.daily_quota as u64 } else { self.default_daily };
        if daily_limit > 0 {
            let (tx, rx) = self.db.get_daily_usage(username).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= daily_limit {
                return Err(QuotaError::DailyExceeded { used, limit: daily_limit });
            }
        }

        // Weekly
        let weekly_limit = if user.weekly_quota > 0 { user.weekly_quota as u64 } else { self.default_weekly };
        if weekly_limit > 0 {
            let (tx, rx) = self.db.get_weekly_usage(username).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= weekly_limit {
                return Err(QuotaError::WeeklyExceeded { used, limit: weekly_limit });
            }
        }

        // Monthly
        if self.default_monthly > 0 {
            let (tx, rx) = self.db.get_monthly_usage(username).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= self.default_monthly {
                return Err(QuotaError::MonthlyExceeded { used, limit: self.default_monthly });
            }
        }

        Ok(())
    }

    /// Check if an IP is allowed to connect (connection count + bandwidth quotas).
    pub fn check_ip(&self, ip: &IpAddr) -> Result<(), QuotaError> {
        // Connection limit
        if self.max_conn_per_ip > 0 {
            let conns = self.active_connections.lock().unwrap();
            let current = conns.get(ip).copied().unwrap_or(0);
            if current >= self.max_conn_per_ip {
                return Err(QuotaError::TooManyConnections {
                    current,
                    limit: self.max_conn_per_ip,
                });
            }
        }

        let ip_str = ip.to_string();

        // IP daily
        if self.ip_daily > 0 {
            let (tx, rx) = self.db.get_ip_daily_usage(&ip_str).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= self.ip_daily {
                return Err(QuotaError::IpDailyExceeded { used, limit: self.ip_daily });
            }
        }

        // IP weekly
        if self.ip_weekly > 0 {
            let (tx, rx) = self.db.get_ip_weekly_usage(&ip_str).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= self.ip_weekly {
                return Err(QuotaError::IpWeeklyExceeded { used, limit: self.ip_weekly });
            }
        }

        // IP monthly
        if self.ip_monthly > 0 {
            let (tx, rx) = self.db.get_ip_monthly_usage(&ip_str).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= self.ip_monthly {
                return Err(QuotaError::IpMonthlyExceeded { used, limit: self.ip_monthly });
            }
        }

        Ok(())
    }

    pub fn connect(&self, ip: &IpAddr) {
        let mut conns = self.active_connections.lock().unwrap();
        *conns.entry(*ip).or_insert(0) += 1;
    }

    pub fn disconnect(&self, ip: &IpAddr) {
        let mut conns = self.active_connections.lock().unwrap();
        if let Some(count) = conns.get_mut(ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                conns.remove(ip);
            }
        }
    }

    /// Record usage after a test completes (both user and IP).
    pub fn record_usage(&self, username: &str, ip: &str, tx_bytes: u64, rx_bytes: u64) {
        if let Err(e) = self.db.record_usage(username, tx_bytes, rx_bytes) {
            tracing::error!("Failed to record user usage for {}: {}", username, e);
        }
        if let Err(e) = self.db.record_ip_usage(ip, tx_bytes, rx_bytes) {
            tracing::error!("Failed to record IP usage for {}: {}", ip, e);
        }
    }

    pub fn max_duration(&self) -> u64 {
        self.max_duration
    }

    pub fn active_connections_count(&self, ip: &IpAddr) -> u32 {
        let conns = self.active_connections.lock().unwrap();
        conns.get(ip).copied().unwrap_or(0)
    }
}
