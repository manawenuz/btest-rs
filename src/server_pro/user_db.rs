//! SQLite-based user database for btest-server-pro.
//!
//! Stores users with credentials, quotas, and usage tracking.

use rusqlite::{Connection, params};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct UserDb {
    conn: Arc<Mutex<Connection>>,
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

impl UserDb {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
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

            CREATE INDEX IF NOT EXISTS idx_usage_user_date ON usage(username, date);
            CREATE INDEX IF NOT EXISTS idx_sessions_peer ON sessions(peer_ip, started_at);
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
        conn.execute(
            "INSERT OR REPLACE INTO users (username, password_hash) VALUES (?1, ?2)",
            params![username, hash],
        )?;
        Ok(())
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
