//! btest-server-pro: MikroTik Bandwidth Test server with multi-user, quotas, and LDAP.
//!
//! This is a superset of the standard `btest` server with additional features:
//! - SQLite user database (--users-db)
//! - Per-user and per-IP bandwidth quotas (daily/weekly)
//! - LDAP/Active Directory authentication (--ldap-url)
//! - Rate limiting for public server deployment
//!
//! Build with: cargo build --release --features pro --bin btest-server-pro

mod user_db;
mod quota;
mod enforcer;
mod server_loop;
mod web;
mod ldap_auth;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "btest-server-pro",
    about = "btest-rs Pro Server: multi-user, quotas, LDAP",
    version,
)]
struct Cli {
    /// Listen port
    #[arg(short = 'P', long = "port", default_value_t = 2000)]
    port: u16,

    /// IPv4 listen address
    #[arg(long = "listen", default_value = "0.0.0.0")]
    listen_addr: String,

    /// IPv6 listen address (optional)
    #[arg(long = "listen6")]
    listen6_addr: Option<String>,

    /// SQLite user database path
    #[arg(long = "users-db", default_value = "btest-users.db")]
    users_db: String,

    /// LDAP server URL (e.g., ldap://dc.example.com)
    #[arg(long = "ldap-url")]
    ldap_url: Option<String>,

    /// LDAP base DN for user search
    #[arg(long = "ldap-base-dn")]
    ldap_base_dn: Option<String>,

    /// LDAP bind DN (for service account)
    #[arg(long = "ldap-bind-dn")]
    ldap_bind_dn: Option<String>,

    /// LDAP bind password
    #[arg(long = "ldap-bind-pass")]
    ldap_bind_pass: Option<String>,

    /// Default daily quota per user in bytes (0 = unlimited)
    #[arg(long = "daily-quota", default_value_t = 0)]
    daily_quota: u64,

    /// Default weekly quota per user in bytes (0 = unlimited)
    #[arg(long = "weekly-quota", default_value_t = 0)]
    weekly_quota: u64,

    /// Default monthly quota per user in bytes (0 = unlimited)
    #[arg(long = "monthly-quota", default_value_t = 0)]
    monthly_quota: u64,

    /// Daily bandwidth limit per IP in bytes (0 = unlimited)
    #[arg(long = "ip-daily", default_value_t = 0)]
    ip_daily: u64,

    /// Weekly bandwidth limit per IP in bytes (0 = unlimited)
    #[arg(long = "ip-weekly", default_value_t = 0)]
    ip_weekly: u64,

    /// Monthly bandwidth limit per IP in bytes (0 = unlimited)
    #[arg(long = "ip-monthly", default_value_t = 0)]
    ip_monthly: u64,

    /// Maximum concurrent connections per IP (0 = unlimited)
    #[arg(long = "max-conn-per-ip", default_value_t = 5)]
    max_conn_per_ip: u32,

    /// Maximum test duration in seconds (0 = unlimited)
    #[arg(long = "max-duration", default_value_t = 300)]
    max_duration: u64,

    /// Daily inbound (client→server) limit per IP in bytes (0 = use --ip-daily)
    #[arg(long = "ip-daily-in", default_value_t = 0)]
    ip_daily_in: u64,

    /// Daily outbound (server→client) limit per IP in bytes (0 = use --ip-daily)
    #[arg(long = "ip-daily-out", default_value_t = 0)]
    ip_daily_out: u64,

    /// Weekly inbound limit per IP in bytes (0 = use --ip-weekly)
    #[arg(long = "ip-weekly-in", default_value_t = 0)]
    ip_weekly_in: u64,

    /// Weekly outbound limit per IP in bytes (0 = use --ip-weekly)
    #[arg(long = "ip-weekly-out", default_value_t = 0)]
    ip_weekly_out: u64,

    /// Monthly inbound limit per IP in bytes (0 = use --ip-monthly)
    #[arg(long = "ip-monthly-in", default_value_t = 0)]
    ip_monthly_in: u64,

    /// Monthly outbound limit per IP in bytes (0 = use --ip-monthly)
    #[arg(long = "ip-monthly-out", default_value_t = 0)]
    ip_monthly_out: u64,

    /// How often to check quotas during a test in seconds
    #[arg(long = "quota-check-interval", default_value_t = 10)]
    quota_check_interval: u64,

    /// Web dashboard port (0 = disabled)
    #[arg(long = "web-port", default_value_t = 8080)]
    web_port: u16,

    /// Shared password for public mode (all users use this password)
    #[arg(long = "shared-password")]
    shared_password: Option<String>,

    /// Use EC-SRP5 authentication
    #[arg(long = "ecsrp5")]
    ecsrp5: bool,

    /// Syslog server address
    #[arg(long = "syslog")]
    syslog: Option<String>,

    /// CSV output file
    #[arg(long = "csv")]
    csv: Option<String>,

    /// Verbose logging
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    verbose: u8,

    /// User management subcommand
    #[command(subcommand)]
    command: Option<UserCommand>,
}

#[derive(clap::Subcommand, Debug)]
enum UserCommand {
    /// Add a user
    #[command(name = "useradd")]
    UserAdd {
        /// Username
        username: String,
        /// Password
        password: String,
    },
    /// Delete a user
    #[command(name = "userdel")]
    UserDel {
        /// Username
        username: String,
    },
    /// List all users
    #[command(name = "userlist")]
    UserList,
    /// Enable/disable a user
    #[command(name = "userset")]
    UserSet {
        /// Username
        username: String,
        /// Enable (true/false)
        #[arg(long)]
        enabled: Option<bool>,
        /// Daily quota in bytes
        #[arg(long)]
        daily: Option<i64>,
        /// Weekly quota in bytes
        #[arg(long)]
        weekly: Option<i64>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = match cli.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .with_target(false)
        .init();

    // Initialize subsystems
    btest_rs::cpu::start_sampler();

    if let Some(ref syslog_addr) = cli.syslog {
        if let Err(e) = btest_rs::syslog_logger::init(syslog_addr) {
            eprintln!("Warning: syslog init failed: {}", e);
        }
    }

    if let Some(ref csv_path) = cli.csv {
        if let Err(e) = btest_rs::csv_output::init(csv_path) {
            eprintln!("Warning: CSV init failed: {}", e);
        }
    }

    // Initialize user database
    let db = user_db::UserDb::open(&cli.users_db)?;
    db.ensure_tables()?;

    // Handle user management subcommands (exit after)
    if let Some(cmd) = &cli.command {
        match cmd {
            UserCommand::UserAdd { username, password } => {
                db.add_user(username, password)?;
                println!("User '{}' added.", username);
                return Ok(());
            }
            UserCommand::UserDel { username } => {
                if db.delete_user(username)? {
                    println!("User '{}' deleted.", username);
                } else {
                    println!("User '{}' not found.", username);
                }
                return Ok(());
            }
            UserCommand::UserList => {
                let users = db.list_users()?;
                if users.is_empty() {
                    println!("No users.");
                } else {
                    println!("{:<20} {:<10} {:<15} {:<15}", "USERNAME", "ENABLED", "DAILY_QUOTA", "WEEKLY_QUOTA");
                    println!("{}", "-".repeat(60));
                    for u in &users {
                        println!("{:<20} {:<10} {:<15} {:<15}",
                            u.username,
                            if u.enabled { "yes" } else { "no" },
                            if u.daily_quota == 0 { "default".to_string() } else { format!("{}B", u.daily_quota) },
                            if u.weekly_quota == 0 { "default".to_string() } else { format!("{}B", u.weekly_quota) },
                        );
                    }
                }
                return Ok(());
            }
            UserCommand::UserSet { username, enabled, daily, weekly } => {
                if let Some(e) = enabled {
                    db.set_user_enabled(username, *e)?;
                    println!("User '{}' enabled={}", username, e);
                }
                if daily.is_some() || weekly.is_some() {
                    let d = daily.unwrap_or(0);
                    let w = weekly.unwrap_or(0);
                    db.set_user_quota(username, d, w, 0)?;
                    println!("User '{}' quota: daily={}, weekly={}", username, d, w);
                }
                return Ok(());
            }
        }
    }

    tracing::info!("User database: {} ({} users)", cli.users_db, db.user_count()?);

    // Initialize LDAP if configured
    if let Some(ref url) = cli.ldap_url {
        tracing::info!("LDAP configured: {}", url);
    }

    // Initialize quota manager
    // Directional flags override combined: --ip-daily-in > --ip-daily > unlimited
    let or_fallback = |specific: u64, combined: u64| if specific > 0 { specific } else { combined };
    let quota_mgr = quota::QuotaManager::new(
        db.clone(),
        cli.daily_quota,
        cli.weekly_quota,
        cli.monthly_quota,
        cli.ip_daily,
        cli.ip_weekly,
        cli.ip_monthly,
        or_fallback(cli.ip_daily_in, cli.ip_daily),
        or_fallback(cli.ip_daily_out, cli.ip_daily),
        or_fallback(cli.ip_weekly_in, cli.ip_weekly),
        or_fallback(cli.ip_weekly_out, cli.ip_weekly),
        or_fallback(cli.ip_monthly_in, cli.ip_monthly),
        or_fallback(cli.ip_monthly_out, cli.ip_monthly),
        cli.max_conn_per_ip,
        cli.max_duration,
    );

    let fmt_q = |v: u64| if v == 0 { "unlimited".to_string() } else { format!("{}B", v) };
    tracing::info!(
        "User quotas: daily={}, weekly={}, monthly={}",
        fmt_q(cli.daily_quota), fmt_q(cli.weekly_quota), fmt_q(cli.monthly_quota),
    );
    tracing::info!(
        "IP quotas: daily={}, weekly={}, monthly={}",
        fmt_q(cli.ip_daily), fmt_q(cli.ip_weekly), fmt_q(cli.ip_monthly),
    );
    tracing::info!(
        "Limits: max_conn_per_ip={}, max_duration={}s",
        cli.max_conn_per_ip, cli.max_duration,
    );

    // Start web dashboard if port > 0
    if cli.web_port > 0 {
        let web_db = db.clone();
        let web_port = cli.web_port;
        tokio::spawn(async move {
            tracing::info!("Web dashboard starting on http://0.0.0.0:{}", web_port);
            let app = web::create_router(web_db);
            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", web_port))
                .await
                .expect("Failed to bind web dashboard port");
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("Web dashboard error: {}", e);
            }
        });
    }

    tracing::info!("btest-server-pro starting on port {}", cli.port);

    let v4 = if cli.listen_addr.eq_ignore_ascii_case("none") { None } else { Some(cli.listen_addr) };
    let v6 = cli.listen6_addr;

    server_loop::run_pro_server(
        cli.port,
        cli.ecsrp5,
        v4, v6,
        db,
        quota_mgr,
        cli.quota_check_interval,
    ).await?;

    Ok(())
}
