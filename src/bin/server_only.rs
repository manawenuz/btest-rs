//! btest-server: minimal bandwidth test server for embedded/OpenWrt systems.
//!
//! Stripped-down server that accepts MikroTik client connections.
//! No client mode, no syslog, no CSV, smaller binary footprint.
//!
//! Build: cargo build --profile release-small --bin btest-server

use clap::Parser;

#[derive(Parser)]
#[command(name = "btest-server", about = "MikroTik Bandwidth Test server", version)]
struct Cli {
    /// Port
    #[arg(short = 'P', long = "port", default_value_t = 2000)]
    port: u16,

    /// IPv4 listen address
    #[arg(long = "listen", default_value = "0.0.0.0")]
    listen_addr: String,

    /// Enable IPv6 listener, optionally on a specific address
    #[arg(long = "listen6", default_missing_value = "::", num_args = 0..=1)]
    listen6_addr: Option<String>,

    /// Username
    #[arg(short = 'a', long = "authuser")]
    auth_user: Option<String>,

    /// Password
    #[arg(short = 'p', long = "authpass")]
    auth_pass: Option<String>,

    /// Use EC-SRP5 authentication
    #[arg(long = "ecsrp5")]
    ecsrp5: bool,

    /// Verbose
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    verbose: u8,
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
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_target(false)
        .init();

    btest_rs::cpu::start_sampler();

    let v4 = if cli.listen_addr.eq_ignore_ascii_case("none") { None } else { Some(cli.listen_addr) };
    let v6 = cli.listen6_addr;

    tracing::info!("btest-server starting on port {}", cli.port);
    btest_rs::server::run_server(cli.port, cli.auth_user, cli.auth_pass, cli.ecsrp5, v4, v6).await?;
    Ok(())
}
