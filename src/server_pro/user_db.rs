//! SQLite-based user database for btest-server-pro.
//!
//! Stores users with credentials, quotas, and usage tracking.

use rusqlite::{Connection, params};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct UserDb {
    conn: Arc<Mutex<Connection>>,
    path: Arc<String>,
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String, // stored as hex of SHA256(username:password)
    pub daily_quota: i64,      // 0 = use default
    pub weekly_quota: i64,     // 0 = use default
    pub enabled: bool,
}

#[derive(Debug)]
pub struct UsageRecord {
    pub username: String,
    pub date: String, // YYYY-MM-DD
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub test_count: u32,
}

/// Per-second bandwidth interval data for graphing.
#[derive(Debug, Clone)]
pub struct IntervalData {
    pub interval_num: i32,
    pub tx_mbps: f64,
    pub rx_mbps: f64,
    pub local_cpu: i32,
    pub remote_cpu: i32,
    pub lost: i64,
}

/// Summary of a single test session.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub protocol: String,
    pub direction: String,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
}

/// Aggregate statistics for an IP address.
#[derive(Debug, Clone)]
pub struct IpStats {
    pub total_tests: u64,
    pub total_inbound: u64,
    pub total_outbound: u64,
    pub avg_tx_mbps: f64,
    pub avg_rx_mbps: f64,
}

impl UserDb {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Arc::new(path.to_string()),
        })
    }

    /// Return the database file path.
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn ensure_tables(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT UNIQUE NOT NULL,
                password_hash TEXT NOT NULL,
                daily_quota INTEGER DEFAULT 0,
                weekly_quota INTEGER DEFAULT 0,
                enabled INTEGER DEFAULT 1,
                created_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL,
                date TEXT NOT NULL,
                tx_bytes INTEGER DEFAULT 0,
                rx_bytes INTEGER DEFAULT 0,
                test_count INTEGER DEFAULT 0,
                UNIQUE(username, date)
            );

            CREATE TABLE IF NOT EXISTS ip_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ip TEXT NOT NULL,
                date TEXT NOT NULL,
                inbound_bytes INTEGER DEFAULT 0,
                outbound_bytes INTEGER DEFAULT 0,
                test_count INTEGER DEFAULT 0,
                UNIQUE(ip, date)
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL,
                peer_ip TEXT NOT NULL,
                started_at TEXT DEFAULT (datetime('now')),
                ended_at TEXT,
                tx_bytes INTEGER DEFAULT 0,
                rx_bytes INTEGER DEFAULT 0,
                protocol TEXT,
                direction TEXT
            );

            CREATE TABLE IF NOT EXISTS test_intervals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                interval_num INTEGER NOT NULL,
                tx_bytes INTEGER DEFAULT 0,
                rx_bytes INTEGER DEFAULT 0,
                tx_mbps REAL DEFAULT 0,
                rx_mbps REAL DEFAULT 0,
                local_cpu INTEGER DEFAULT 0,
                remote_cpu INTEGER DEFAULT 0,
                lost_packets INTEGER DEFAULT 0,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );

            CREATE INDEX IF NOT EXISTS idx_usage_user_date ON usage(username, date);
            CREATE INDEX IF NOT EXISTS idx_ip_usage_date ON ip_usage(ip, date);
            CREATE INDEX IF NOT EXISTS idx_sessions_peer ON sessions(peer_ip, started_at);
            CREATE INDEX IF NOT EXISTS idx_intervals_session ON test_intervals(session_id);
        ")?;
        Ok(())
    }

    pub fn user_count(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        Ok(count as u64)
    }

    pub fn add_user(&self, username: &str, password: &str) -> anyhow::Result<()> {
        let hash = hash_password(username, password);
        let conn = self.conn.lock().unwrap();
        // Ensure password_raw column exists (migration for older databases)
        let _ = conn.execute("ALTER TABLE users ADD COLUMN password_raw TEXT DEFAULT ''", []);
        conn.execute(
            "INSERT OR REPLACE INTO users (username, password_hash, password_raw) VALUES (?1, ?2, ?3)",
            params![username, hash, password],
        )?;
        Ok(())
    }

    /// Get the raw password for MD5 challenge-response auth.
    pub fn get_password(&self, username: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT password_raw FROM users WHERE username = ?1 AND enabled = 1",
            params![username],
            |row| row.get::<_, String>(0),
        ).optional()?;
        Ok(result)
    }

    pub fn get_user(&self, username: &str) -> anyhow::Result<Option<User>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, username, password_hash, daily_quota, weekly_quota, enabled FROM users WHERE username = ?1"
        )?;
        let user = stmt.query_row(params![username], |row| {
            Ok(User {
                id: row.get(0)?,
                username: row.get(1)?,
                password_hash: row.get(2)?,
                daily_quota: row.get(3)?,
                weekly_quota: row.get(4)?,
                enabled: row.get::<_, i32>(5)? != 0,
            })
        }).optional()?;
        Ok(user)
    }

    pub fn verify_password(&self, username: &str, password: &str) -> anyhow::Result<bool> {
        let expected = hash_password(username, password);
        match self.get_user(username)? {
            Some(user) => Ok(user.enabled && user.password_hash == expected),
            None => Ok(false),
        }
    }

    pub fn record_usage(&self, username: &str, tx_bytes: u64, rx_bytes: u64) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let today = chrono_date_today();
        conn.execute(
            "INSERT INTO usage (username, date, tx_bytes, rx_bytes, test_count)
             VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT(username, date) DO UPDATE SET
                tx_bytes = tx_bytes + ?3,
                rx_bytes = rx_bytes + ?4,
                test_count = test_count + 1",
            params![username, today, tx_bytes as i64, rx_bytes as i64],
        )?;
        Ok(())
    }

    pub fn get_daily_usage(&self, username: &str) -> anyhow::Result<(u64, u64)> {
        let conn = self.conn.lock().unwrap();
        let today = chrono_date_today();
        let result = conn.query_row(
            "SELECT COALESCE(SUM(tx_bytes),0), COALESCE(SUM(rx_bytes),0) FROM usage WHERE username = ?1 AND date = ?2",
            params![username, today],
            |row| {
                let a: i64 = row.get(0)?;
                let b: i64 = row.get(1)?;
                Ok((a as u64, b as u64))
            },
        )?;
        Ok(result)
    }

    pub fn get_weekly_usage(&self, username: &str) -> anyhow::Result<(u64, u64)> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT COALESCE(SUM(tx_bytes),0), COALESCE(SUM(rx_bytes),0) FROM usage
             WHERE username = ?1 AND date >= date('now', '-7 days')",
            params![username],
            |row| {
                let a: i64 = row.get(0)?;
                let b: i64 = row.get(1)?;
                Ok((a as u64, b as u64))
            },
        )?;
        Ok(result)
    }

    pub fn get_monthly_usage(&self, username: &str) -> anyhow::Result<(u64, u64)> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT COALESCE(SUM(tx_bytes),0), COALESCE(SUM(rx_bytes),0) FROM usage
             WHERE username = ?1 AND date >= date('now', '-30 days')",
            params![username],
            |row| {
                let a: i64 = row.get(0)?;
                let b: i64 = row.get(1)?;
                Ok((a as u64, b as u64))
            },
        )?;
        Ok(result)
    }

    // --- Per-IP usage tracking ---

    pub fn record_ip_usage(&self, ip: &str, tx_bytes: u64, rx_bytes: u64) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let today = chrono_date_today();
        // From the server's perspective: inbound = data coming FROM the client (rx),
        // outbound = data going TO the client (tx).
        let inbound = rx_bytes;
        let outbound = tx_bytes;
        conn.execute(
            "INSERT INTO ip_usage (ip, date, inbound_bytes, outbound_bytes, test_count)
             VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT(ip, date) DO UPDATE SET
                inbound_bytes = inbound_bytes + ?3,
                outbound_bytes = outbound_bytes + ?4,
                test_count = test_count + 1",
            params![ip, today, inbound as i64, outbound as i64],
        )?;
        Ok(())
    }

    pub fn get_ip_daily_usage(&self, ip: &str) -> anyhow::Result<(u64, u64)> {
        let conn = self.conn.lock().unwrap();
        let today = chrono_date_today();
        let result = conn.query_row(
            "SELECT COALESCE(SUM(inbound_bytes),0), COALESCE(SUM(outbound_bytes),0) FROM ip_usage WHERE ip = ?1 AND date = ?2",
            params![ip, today],
            |row| {
                let inbound: i64 = row.get(0)?;
                let outbound: i64 = row.get(1)?;
                Ok((inbound as u64, outbound as u64))
            },
        )?;
        Ok(result)
    }

    pub fn get_ip_weekly_usage(&self, ip: &str) -> anyhow::Result<(u64, u64)> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT COALESCE(SUM(inbound_bytes),0), COALESCE(SUM(outbound_bytes),0) FROM ip_usage
             WHERE ip = ?1 AND date >= date('now', '-7 days')",
            params![ip],
            |row| {
                let inbound: i64 = row.get(0)?;
                let outbound: i64 = row.get(1)?;
                Ok((inbound as u64, outbound as u64))
            },
        )?;
        Ok(result)
    }

    pub fn get_ip_monthly_usage(&self, ip: &str) -> anyhow::Result<(u64, u64)> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT COALESCE(SUM(inbound_bytes),0), COALESCE(SUM(outbound_bytes),0) FROM ip_usage
             WHERE ip = ?1 AND date >= date('now', '-30 days')",
            params![ip],
            |row| {
                let inbound: i64 = row.get(0)?;
                let outbound: i64 = row.get(1)?;
                Ok((inbound as u64, outbound as u64))
            },
        )?;
        Ok(result)
    }

    // --- Per-IP directional usage (single-column queries) ---

    /// Record inbound-only IP usage (data coming FROM the client).
    pub fn record_ip_inbound_usage(&self, ip: &str, bytes: u64) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let today = chrono_date_today();
        conn.execute(
            "INSERT INTO ip_usage (ip, date, inbound_bytes, test_count)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(ip, date) DO UPDATE SET
                inbound_bytes = inbound_bytes + ?3",
            params![ip, today, bytes as i64],
        )?;
        Ok(())
    }

    /// Record outbound-only IP usage (data going TO the client).
    pub fn record_ip_outbound_usage(&self, ip: &str, bytes: u64) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let today = chrono_date_today();
        conn.execute(
            "INSERT INTO ip_usage (ip, date, outbound_bytes, test_count)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(ip, date) DO UPDATE SET
                outbound_bytes = outbound_bytes + ?3",
            params![ip, today, bytes as i64],
        )?;
        Ok(())
    }

    /// Get daily inbound bytes for an IP.
    pub fn get_ip_daily_inbound(&self, ip: &str) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let today = chrono_date_today();
        let result: i64 = conn.query_row(
            "SELECT COALESCE(SUM(inbound_bytes),0) FROM ip_usage WHERE ip = ?1 AND date = ?2",
            params![ip, today],
            |row| row.get(0),
        )?;
        Ok(result as u64)
    }

    /// Get weekly inbound bytes for an IP.
    pub fn get_ip_weekly_inbound(&self, ip: &str) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let result: i64 = conn.query_row(
            "SELECT COALESCE(SUM(inbound_bytes),0) FROM ip_usage WHERE ip = ?1 AND date >= date('now', '-7 days')",
            params![ip],
            |row| row.get(0),
        )?;
        Ok(result as u64)
    }

    /// Get monthly inbound bytes for an IP.
    pub fn get_ip_monthly_inbound(&self, ip: &str) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let result: i64 = conn.query_row(
            "SELECT COALESCE(SUM(inbound_bytes),0) FROM ip_usage WHERE ip = ?1 AND date >= date('now', '-30 days')",
            params![ip],
            |row| row.get(0),
        )?;
        Ok(result as u64)
    }

    /// Get daily outbound bytes for an IP.
    pub fn get_ip_daily_outbound(&self, ip: &str) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let today = chrono_date_today();
        let result: i64 = conn.query_row(
            "SELECT COALESCE(SUM(outbound_bytes),0) FROM ip_usage WHERE ip = ?1 AND date = ?2",
            params![ip, today],
            |row| row.get(0),
        )?;
        Ok(result as u64)
    }

    /// Get weekly outbound bytes for an IP.
    pub fn get_ip_weekly_outbound(&self, ip: &str) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let result: i64 = conn.query_row(
            "SELECT COALESCE(SUM(outbound_bytes),0) FROM ip_usage WHERE ip = ?1 AND date >= date('now', '-7 days')",
            params![ip],
            |row| row.get(0),
        )?;
        Ok(result as u64)
    }

    /// Get monthly outbound bytes for an IP.
    pub fn get_ip_monthly_outbound(&self, ip: &str) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let result: i64 = conn.query_row(
            "SELECT COALESCE(SUM(outbound_bytes),0) FROM ip_usage WHERE ip = ?1 AND date >= date('now', '-30 days')",
            params![ip],
            |row| row.get(0),
        )?;
        Ok(result as u64)
    }

    // --- Session tracking ---

    pub fn start_session(&self, username: &str, peer_ip: &str, protocol: &str, direction: &str) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (username, peer_ip, protocol, direction) VALUES (?1, ?2, ?3, ?4)",
            params![username, peer_ip, protocol, direction],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn end_session(&self, session_id: i64, tx_bytes: u64, rx_bytes: u64) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET ended_at = datetime('now'), tx_bytes = ?1, rx_bytes = ?2 WHERE id = ?3",
            params![tx_bytes as i64, rx_bytes as i64, session_id],
        )?;
        Ok(())
    }

    // --- Per-second interval tracking ---

    /// Record a single per-second interval data point for a session.
    #[allow(clippy::too_many_arguments)]
    pub fn record_test_interval(
        &self,
        session_id: i64,
        interval_num: i32,
        tx_bytes: u64,
        rx_bytes: u64,
        tx_mbps: f64,
        rx_mbps: f64,
        local_cpu: i32,
        remote_cpu: i32,
        lost: i64,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO test_intervals (session_id, interval_num, tx_bytes, rx_bytes, tx_mbps, rx_mbps, local_cpu, remote_cpu, lost_packets)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session_id,
                interval_num,
                tx_bytes as i64,
                rx_bytes as i64,
                tx_mbps,
                rx_mbps,
                local_cpu,
                remote_cpu,
                lost,
            ],
        )?;
        Ok(())
    }

    /// Retrieve all interval data points for a given session, ordered by interval number.
    pub fn get_session_intervals(&self, session_id: i64) -> anyhow::Result<Vec<IntervalData>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT interval_num, tx_mbps, rx_mbps, local_cpu, remote_cpu, lost_packets
             FROM test_intervals WHERE session_id = ?1 ORDER BY interval_num"
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(IntervalData {
                interval_num: row.get(0)?,
                tx_mbps: row.get(1)?,
                rx_mbps: row.get(2)?,
                local_cpu: row.get(3)?,
                remote_cpu: row.get(4)?,
                lost: row.get(5)?,
            })
        })?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    }

    /// Return the last N sessions for a given IP address, most recent first.
    pub fn get_ip_sessions(&self, ip: &str, limit: u32) -> anyhow::Result<Vec<SessionSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, started_at, ended_at, protocol, direction, tx_bytes, rx_bytes
             FROM sessions WHERE peer_ip = ?1 ORDER BY started_at DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![ip, limit], |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                started_at: row.get(1)?,
                ended_at: row.get(2)?,
                protocol: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                direction: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                tx_bytes: row.get::<_, i64>(5).map(|v| v as u64)?,
                rx_bytes: row.get::<_, i64>(6).map(|v| v as u64)?,
            })
        })?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    }

    /// Return aggregate statistics for an IP address across all sessions.
    pub fn get_ip_stats(&self, ip: &str) -> anyhow::Result<IpStats> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT
                COUNT(*) as total_tests,
                COALESCE(SUM(inbound_bytes), 0) as total_inbound,
                COALESCE(SUM(outbound_bytes), 0) as total_outbound
             FROM ip_usage WHERE ip = ?1",
            params![ip],
            |row| {
                let total_tests: i64 = row.get(0)?;
                let total_inbound: i64 = row.get(1)?;
                let total_outbound: i64 = row.get(2)?;
                Ok((total_tests as u64, total_inbound as u64, total_outbound as u64))
            },
        )?;

        // Compute average Mbps from test_intervals joined through sessions
        let (avg_tx, avg_rx) = conn.query_row(
            "SELECT
                COALESCE(AVG(ti.tx_mbps), 0.0),
                COALESCE(AVG(ti.rx_mbps), 0.0)
             FROM test_intervals ti
             INNER JOIN sessions s ON ti.session_id = s.id
             WHERE s.peer_ip = ?1",
            params![ip],
            |row| {
                let avg_tx: f64 = row.get(0)?;
                let avg_rx: f64 = row.get(1)?;
                Ok((avg_tx, avg_rx))
            },
        )?;

        Ok(IpStats {
            total_tests: result.0,
            total_inbound: result.1,
            total_outbound: result.2,
            avg_tx_mbps: avg_tx,
            avg_rx_mbps: avg_rx,
        })
    }

    pub fn delete_user(&self, username: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM users WHERE username = ?1", params![username])?;
        Ok(rows > 0)
    }

    pub fn set_user_enabled(&self, username: &str, enabled: bool) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE users SET enabled = ?1 WHERE username = ?2",
            params![enabled as i32, username],
        )?;
        Ok(())
    }

    pub fn set_user_quota(&self, username: &str, daily: i64, weekly: i64, monthly: i64) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE users SET daily_quota = ?1, weekly_quota = ?2 WHERE username = ?3",
            params![daily, weekly, username],
        )?;
        Ok(())
    }

    pub fn list_users(&self) -> anyhow::Result<Vec<User>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, username, password_hash, daily_quota, weekly_quota, enabled FROM users ORDER BY username"
        )?;
        let users = stmt.query_map([], |row| {
            Ok(User {
                id: row.get(0)?,
                username: row.get(1)?,
                password_hash: row.get(2)?,
                daily_quota: row.get(3)?,
                weekly_quota: row.get(4)?,
                enabled: row.get::<_, i32>(5)? != 0,
            })
        })?.filter_map(|r| r.ok()).collect();
        Ok(users)
    }
}

fn hash_password(username: &str, password: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", username, password).as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

fn chrono_date_today() -> String {
    // Simple date without chrono crate
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = secs / 86400;
    let mut y = 1970u64;
    let mut remaining = days;
    loop {
        let leap = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < leap { break; }
        remaining -= leap;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in_months = [31u64, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0usize;
    for i in 0..12 {
        if remaining < days_in_months[i] { m = i; break; }
        remaining -= days_in_months[i];
    }
    format!("{:04}-{:02}-{:02}", y, m + 1, remaining + 1)
}

// Re-export for use by rusqlite
use rusqlite::OptionalExtension;
