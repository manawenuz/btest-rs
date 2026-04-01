//! btest-client: minimal bandwidth test client for embedded/OpenWrt systems.
//!
//! Stripped-down client that connects to MikroTik btest servers.
//! No server mode, no syslog, smaller binary footprint.
//!
//! Build: cargo build --profile release-small --bin btest-client

use clap::Parser;
use std::sync::atomic::Ordering;

#[derive(Parser)]
#[command(name = "btest-client", about = "MikroTik Bandwidth Test client", version)]
struct Cli {
    /// Server address to connect to
    #[arg(short = 'c', long = "client", required = true)]
    host: String,

    /// Transmit data (upload)
    #[arg(short = 't', long = "transmit")]
    transmit: bool,

    /// Receive data (download)
    #[arg(short = 'r', long = "receive")]
    receive: bool,

    /// Use UDP
    #[arg(short = 'u', long = "udp")]
    udp: bool,

    /// Bandwidth limit (e.g., 100M)
    #[arg(short = 'b', long = "bandwidth")]
    bandwidth: Option<String>,

    /// Port
    #[arg(short = 'P', long = "port", default_value_t = 2000)]
    port: u16,

    /// Username
    #[arg(short = 'a', long = "authuser")]
    auth_user: Option<String>,

    /// Password
    #[arg(short = 'p', long = "authpass")]
    auth_pass: Option<String>,

    /// NAT mode
    #[arg(short = 'n', long = "nat")]
    nat: bool,

    /// Duration in seconds (0=unlimited)
    #[arg(short = 'd', long = "duration", default_value_t = 0)]
    duration: u64,

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

    if !cli.transmit && !cli.receive {
        eprintln!("Error: specify -t (transmit) and/or -r (receive)");
        std::process::exit(1);
    }

    let direction = match (cli.transmit, cli.receive) {
        (true, false) => btest_rs::protocol::CMD_DIR_RX,
        (false, true) => btest_rs::protocol::CMD_DIR_TX,
        (true, true) => btest_rs::protocol::CMD_DIR_BOTH,
        _ => unreachable!(),
    };

    let bw = match &cli.bandwidth {
        Some(b) => btest_rs::bandwidth::parse_bandwidth(b)?,
        None => 0,
    };

    let (tx_speed, rx_speed) = match direction {
        btest_rs::protocol::CMD_DIR_TX => (bw, 0),
        btest_rs::protocol::CMD_DIR_RX => (0, bw),
        _ => (bw, bw),
    };

    let state = btest_rs::bandwidth::BandwidthState::new();
    let state_clone = state.clone();

    let host = cli.host.clone();
    let client_fut = btest_rs::client::run_client(
        &host, cli.port, direction, cli.udp,
        tx_speed, rx_speed,
        cli.auth_user, cli.auth_pass, cli.nat,
        state_clone,
    );

    if cli.duration > 0 {
        match tokio::time::timeout(
            std::time::Duration::from_secs(cli.duration),
            client_fut,
        ).await {
            Ok(r) => { let _ = r?; }
            Err(_) => {
                state.running.store(false, Ordering::SeqCst);
            }
        }
    } else {
        let _ = client_fut.await?;
    }

    Ok(())
}
