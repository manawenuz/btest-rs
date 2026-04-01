//! Bandwidth quota management for btest-server-pro.
//!
//! Enforces per-user and per-IP bandwidth limits (daily/weekly/monthly),
//! with separate tracking for inbound (client-to-server) and outbound
//! (server-to-client) directions.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use super::user_db::UserDb;

/// Traffic direction for bandwidth tests.
///
/// From the **server's** perspective:
/// - `Inbound` = client sends data to us (client TX, server RX)
/// - `Outbound` = we send data to the client (server TX, client RX)
/// - `Both` = bidirectional test
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Inbound,
    Outbound,
    Both,
}

#[derive(Clone)]
pub struct QuotaManager {
    db: UserDb,
    /// Per-user defaults (0 = unlimited)
    default_daily: u64,
    default_weekly: u64,
    default_monthly: u64,
    /// Per-IP combined (inbound + outbound) limits (0 = unlimited) — for abuse prevention
    ip_daily: u64,
    ip_weekly: u64,
    ip_monthly: u64,
    /// Per-IP directional limits (0 = unlimited)
    ip_daily_inbound: u64,
    ip_daily_outbound: u64,
    ip_weekly_inbound: u64,
    ip_weekly_outbound: u64,
    ip_monthly_inbound: u64,
    ip_monthly_outbound: u64,
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
    /// Combined (inbound + outbound) IP daily limit exceeded.
    IpDailyExceeded { used: u64, limit: u64 },
    /// Combined (inbound + outbound) IP weekly limit exceeded.
    IpWeeklyExceeded { used: u64, limit: u64 },
    /// Combined (inbound + outbound) IP monthly limit exceeded.
    IpMonthlyExceeded { used: u64, limit: u64 },
    /// Per-direction IP daily limits.
    IpInboundDailyExceeded { used: u64, limit: u64 },
    IpOutboundDailyExceeded { used: u64, limit: u64 },
    /// Per-direction IP weekly limits.
    IpInboundWeeklyExceeded { used: u64, limit: u64 },
    IpOutboundWeeklyExceeded { used: u64, limit: u64 },
    /// Per-direction IP monthly limits.
    IpInboundMonthlyExceeded { used: u64, limit: u64 },
    IpOutboundMonthlyExceeded { used: u64, limit: u64 },
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
            Self::IpInboundDailyExceeded { used, limit } =>
                write!(f, "IP inbound daily quota exceeded: {}/{} bytes", used, limit),
            Self::IpOutboundDailyExceeded { used, limit } =>
                write!(f, "IP outbound daily quota exceeded: {}/{} bytes", used, limit),
            Self::IpInboundWeeklyExceeded { used, limit } =>
                write!(f, "IP inbound weekly quota exceeded: {}/{} bytes", used, limit),
            Self::IpOutboundWeeklyExceeded { used, limit } =>
                write!(f, "IP outbound weekly quota exceeded: {}/{} bytes", used, limit),
            Self::IpInboundMonthlyExceeded { used, limit } =>
                write!(f, "IP inbound monthly quota exceeded: {}/{} bytes", used, limit),
            Self::IpOutboundMonthlyExceeded { used, limit } =>
                write!(f, "IP outbound monthly quota exceeded: {}/{} bytes", used, limit),
            Self::TooManyConnections { current, limit } =>
                write!(f, "Too many connections from this IP: {}/{}", current, limit),
            Self::UserDisabled => write!(f, "User account is disabled"),
            Self::UserNotFound => write!(f, "User not found"),
        }
    }
}

impl QuotaManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: UserDb,
        default_daily: u64,
        default_weekly: u64,
        default_monthly: u64,
        ip_daily: u64,
        ip_weekly: u64,
        ip_monthly: u64,
        ip_daily_inbound: u64,
        ip_daily_outbound: u64,
        ip_weekly_inbound: u64,
        ip_weekly_outbound: u64,
        ip_monthly_inbound: u64,
        ip_monthly_outbound: u64,
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
            ip_daily_inbound,
            ip_daily_outbound,
            ip_weekly_inbound,
            ip_weekly_outbound,
            ip_monthly_inbound,
            ip_monthly_outbound,
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

    /// Check if an IP is allowed to connect, considering both combined and
    /// directional bandwidth quotas.
    ///
    /// The `direction` parameter indicates which direction the test will use.
    /// For `Direction::Both`, both inbound and outbound directional limits are
    /// checked. Combined (total) limits are always checked regardless of
    /// direction.
    pub fn check_ip(&self, ip: &IpAddr, direction: Direction) -> Result<(), QuotaError> {
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

        // --- Combined (inbound + outbound) limits ---
        self.check_ip_combined(&ip_str)?;

        // --- Directional limits ---
        let check_inbound = matches!(direction, Direction::Inbound | Direction::Both);
        let check_outbound = matches!(direction, Direction::Outbound | Direction::Both);

        if check_inbound {
            self.check_ip_inbound(&ip_str)?;
        }
        if check_outbound {
            self.check_ip_outbound(&ip_str)?;
        }

        Ok(())
    }

    /// Check combined (total inbound + outbound) IP limits.
    fn check_ip_combined(&self, ip_str: &str) -> Result<(), QuotaError> {
        // IP daily (combined)
        if self.ip_daily > 0 {
            let (tx, rx) = self.db.get_ip_daily_usage(ip_str).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= self.ip_daily {
                return Err(QuotaError::IpDailyExceeded { used, limit: self.ip_daily });
            }
        }

        // IP weekly (combined)
        if self.ip_weekly > 0 {
            let (tx, rx) = self.db.get_ip_weekly_usage(ip_str).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= self.ip_weekly {
                return Err(QuotaError::IpWeeklyExceeded { used, limit: self.ip_weekly });
            }
        }

        // IP monthly (combined)
        if self.ip_monthly > 0 {
            let (tx, rx) = self.db.get_ip_monthly_usage(ip_str).unwrap_or((0, 0));
            let used = tx + rx;
            if used >= self.ip_monthly {
                return Err(QuotaError::IpMonthlyExceeded { used, limit: self.ip_monthly });
            }
        }

        Ok(())
    }

    /// Check inbound-only (client sends to us) IP limits.
    fn check_ip_inbound(&self, ip_str: &str) -> Result<(), QuotaError> {
        // Daily inbound
        if self.ip_daily_inbound > 0 {
            let used = self.db.get_ip_daily_inbound(ip_str).unwrap_or(0);
            if used >= self.ip_daily_inbound {
                return Err(QuotaError::IpInboundDailyExceeded {
                    used,
                    limit: self.ip_daily_inbound,
                });
            }
        }

        // Weekly inbound
        if self.ip_weekly_inbound > 0 {
            let used = self.db.get_ip_weekly_inbound(ip_str).unwrap_or(0);
            if used >= self.ip_weekly_inbound {
                return Err(QuotaError::IpInboundWeeklyExceeded {
                    used,
                    limit: self.ip_weekly_inbound,
                });
            }
        }

        // Monthly inbound
        if self.ip_monthly_inbound > 0 {
            let used = self.db.get_ip_monthly_inbound(ip_str).unwrap_or(0);
            if used >= self.ip_monthly_inbound {
                return Err(QuotaError::IpInboundMonthlyExceeded {
                    used,
                    limit: self.ip_monthly_inbound,
                });
            }
        }

        Ok(())
    }

    /// Check outbound-only (we send to client) IP limits.
    fn check_ip_outbound(&self, ip_str: &str) -> Result<(), QuotaError> {
        // Daily outbound
        if self.ip_daily_outbound > 0 {
            let used = self.db.get_ip_daily_outbound(ip_str).unwrap_or(0);
            if used >= self.ip_daily_outbound {
                return Err(QuotaError::IpOutboundDailyExceeded {
                    used,
                    limit: self.ip_daily_outbound,
                });
            }
        }

        // Weekly outbound
        if self.ip_weekly_outbound > 0 {
            let used = self.db.get_ip_weekly_outbound(ip_str).unwrap_or(0);
            if used >= self.ip_weekly_outbound {
                return Err(QuotaError::IpOutboundWeeklyExceeded {
                    used,
                    limit: self.ip_weekly_outbound,
                });
            }
        }

        // Monthly outbound
        if self.ip_monthly_outbound > 0 {
            let used = self.db.get_ip_monthly_outbound(ip_str).unwrap_or(0);
            if used >= self.ip_monthly_outbound {
                return Err(QuotaError::IpOutboundMonthlyExceeded {
                    used,
                    limit: self.ip_monthly_outbound,
                });
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

    /// Record usage after a test completes (both user and IP), with separate
    /// inbound and outbound byte counts.
    ///
    /// - `inbound_bytes`: bytes the client sent to us (server RX).
    /// - `outbound_bytes`: bytes we sent to the client (server TX).
    ///
    /// Both the combined user/IP usage and directional IP usage are recorded.
    pub fn record_usage(
        &self,
        username: &str,
        ip: &str,
        inbound_bytes: u64,
        outbound_bytes: u64,
    ) {
        // Record combined user usage (tx/rx from the server's perspective:
        // tx = outbound, rx = inbound).
        if let Err(e) = self.db.record_usage(username, outbound_bytes, inbound_bytes) {
            tracing::error!("Failed to record user usage for {}: {}", username, e);
        }

        // Record combined IP usage.
        if let Err(e) = self.db.record_ip_usage(ip, outbound_bytes, inbound_bytes) {
            tracing::error!("Failed to record IP usage for {}: {}", ip, e);
        }

        // Record directional IP usage for the new per-direction columns.
        if let Err(e) = self.db.record_ip_inbound_usage(ip, inbound_bytes) {
            tracing::error!("Failed to record IP inbound usage for {}: {}", ip, e);
        }
        if let Err(e) = self.db.record_ip_outbound_usage(ip, outbound_bytes) {
            tracing::error!("Failed to record IP outbound usage for {}: {}", ip, e);
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
