mod auth;
mod bandwidth;
mod client;
mod protocol;
mod server;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::protocol::*;

#[derive(Parser, Debug)]
#[command(
    name = "btest",
    about = "MikroTik Bandwidth Test (btest) - server and client",
    version,
    long_about = "Compatible bandwidth testing tool for MikroTik RouterOS devices.\n\
                  Supports TCP and UDP modes with optional authentication."
)]
struct Cli {
    /// Run in server mode
    #[arg(short = 's', long = "server", conflicts_with = "client")]
    server: bool,

    /// Run in client mode, connecting to the specified host
    #[arg(short = 'c', long = "client", conflicts_with = "server")]
    client: Option<String>,

    /// Client transmits data (upload test)
    #[arg(short = 't', long = "transmit")]
    transmit: bool,

    /// Client receives data (download test)
    #[arg(short = 'r', long = "receive")]
    receive: bool,

    /// Use UDP instead of TCP
    #[arg(short = 'u', long = "udp")]
    udp: bool,

    /// Target bandwidth (e.g., 100M, 1G, 500K)
    #[arg(short = 'b', long = "bandwidth")]
    bandwidth: Option<String>,

    /// Listen/connect port (default: 2000)
    #[arg(short = 'P', long = "port", default_value_t = BTEST_PORT)]
    port: u16,

    /// Authentication username
    #[arg(short = 'a', long = "authuser")]
    auth_user: Option<String>,

    /// Authentication password
    #[arg(short = 'p', long = "authpass")]
    auth_pass: Option<String>,

    /// NAT mode - send probe packet to open firewall
    #[arg(short = 'n', long = "nat")]
    nat: bool,

    /// Verbose logging (repeat for more: -v, -vv, -vvv)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Set up logging based on verbosity
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

    if cli.server {
        // Server mode
        tracing::info!("Starting btest server on port {}", cli.port);
        server::run_server(cli.port, cli.auth_user, cli.auth_pass).await?;
    } else if let Some(host) = cli.client {
        // Client mode - must specify at least one direction
        if !cli.transmit && !cli.receive {
            eprintln!("Error: Client mode requires at least one of -t (transmit) or -r (receive)");
            std::process::exit(1);
        }

        // Direction tells SERVER what to do (C client convention):
        //   client transmit → CMD_DIR_RX (server receives)
        //   client receive  → CMD_DIR_TX (server transmits)
        let direction = match (cli.transmit, cli.receive) {
            (true, false) => CMD_DIR_RX,
            (false, true) => CMD_DIR_TX,
            (true, true) => CMD_DIR_BOTH,
            _ => unreachable!(),
        };

        let bw = match &cli.bandwidth {
            Some(b) => bandwidth::parse_bandwidth(b)?,
            None => 0,
        };

        // For client: local_tx_speed controls upload, remote_tx_speed controls download
        let (tx_speed, rx_speed) = match direction {
            CMD_DIR_TX => (bw, 0),
            CMD_DIR_RX => (0, bw),
            CMD_DIR_BOTH => (bw, bw),
            _ => (0, 0),
        };

        client::run_client(
            &host,
            cli.port,
            direction,
            cli.udp,
            tx_speed,
            rx_speed,
            cli.auth_user,
            cli.auth_pass,
            cli.nat,
        )
        .await?;
    } else {
        eprintln!("Error: Must specify either -s (server) or -c <host> (client)");
        eprintln!("Run with --help for usage information.");
        std::process::exit(1);
    }

    Ok(())
}
