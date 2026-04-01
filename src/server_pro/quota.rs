//! Bandwidth quota management for btest-server-pro.
//!
//! Enforces per-user and per-IP bandwidth limits.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use super::user_db::UserDb;

#[derive(Clone)]
pub struct QuotaManager {
    db: UserDb,
    default_daily: u64,
    default_weekly: u64,
    max_conn_per_ip: u32,
    max_duration: u64,
    active_connections: Arc<Mutex<HashMap<IpAddr, u32>>>,
}

#[derive(Debug)]
pub enum QuotaError {
    DailyExceeded { used: u64, limit: u64 },
    WeeklyExceeded { used: u64, limit: u64 },
    TooManyConnections { current: u32, limit: u32 },
    UserDisabled,
    UserNotFound,
}

impl std::fmt::Display for QuotaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DailyExceeded { used, limit } =>
                write!(f, "Daily quota exceeded: {}/{} bytes", used, limit),
            Self::WeeklyExceeded { used, limit } =>
                write!(f, "Weekly quota exceeded: {}/{} bytes", used, limit),
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
        max_conn_per_ip: u32,
        max_duration: u64,
    ) -> Self {
        Self {
            db,
            default_daily,
            default_weekly,
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

        // Check daily quota
        let daily_limit = if user.daily_quota > 0 { user.daily_quota as u64 } else { self.default_daily };
        if daily_limit > 0 {
            let (tx, rx) = self.db.get_daily_usage(username).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= daily_limit {
                return Err(QuotaError::DailyExceeded { used, limit: daily_limit });
            }
        }

        // Check weekly quota
        let weekly_limit = if user.weekly_quota > 0 { user.weekly_quota as u64 } else { self.default_weekly };
        if weekly_limit > 0 {
            let (tx, rx) = self.db.get_weekly_usage(username).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= weekly_limit {
                return Err(QuotaError::WeeklyExceeded { used, limit: weekly_limit });
            }
        }

        Ok(())
    }

    /// Check if an IP is allowed to connect.
    pub fn check_ip(&self, ip: &IpAddr) -> Result<(), QuotaError> {
        if self.max_conn_per_ip == 0 {
            return Ok(());
        }
        let conns = self.active_connections.lock().unwrap();
        let current = conns.get(ip).copied().unwrap_or(0);
        if current >= self.max_conn_per_ip {
            return Err(QuotaError::TooManyConnections {
                current,
                limit: self.max_conn_per_ip,
            });
        }
        Ok(())
    }

    /// Register an active connection from an IP.
    pub fn connect(&self, ip: &IpAddr) {
        let mut conns = self.active_connections.lock().unwrap();
        *conns.entry(*ip).or_insert(0) += 1;
    }

    /// Unregister a connection from an IP.
    pub fn disconnect(&self, ip: &IpAddr) {
        let mut conns = self.active_connections.lock().unwrap();
        if let Some(count) = conns.get_mut(ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                conns.remove(ip);
            }
        }
    }

    /// Record usage after a test completes.
    pub fn record_usage(&self, username: &str, tx_bytes: u64, rx_bytes: u64) {
        if let Err(e) = self.db.record_usage(username, tx_bytes, rx_bytes) {
            tracing::error!("Failed to record usage for {}: {}", username, e);
        }
    }

    /// Get the maximum test duration in seconds.
    pub fn max_duration(&self) -> u64 {
        self.max_duration
    }
}
