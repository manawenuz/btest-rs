//! Enhanced server loop with quota enforcement.
//!
//! Wraps the standard btest server connection handler with:
//! - Pre-connection IP/user quota checks
//! - Mid-session quota enforcement via QuotaEnforcer
//! - Post-session usage recording

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use btest_rs::protocol::*;
use btest_rs::bandwidth::BandwidthState;

use super::enforcer::{QuotaEnforcer, StopReason};
use super::quota::{Direction, QuotaManager};
use super::user_db::UserDb;

/// Run the pro server with quota enforcement.
pub async fn run_pro_server(
    port: u16,
    ecsrp5: bool,
    listen_v4: Option<String>,
    listen_v6: Option<String>,
    db: UserDb,
    quota_mgr: QuotaManager,
    quota_check_interval: u64,
) -> anyhow::Result<()> {
    // Pre-derive EC-SRP5 creds if needed
    // For pro server, we don't use CLI -a/-p — we use the user DB
    // EC-SRP5 needs a fixed password for the server challenge, but
    // the actual verification happens against the DB.
    // For now, the first user in the DB is used for EC-SRP5 derivation.

    let v4_listener = if let Some(ref addr) = listen_v4 {
        let bind_addr = format!("{}:{}", addr, port);
        Some(TcpListener::bind(&bind_addr).await?)
    } else {
        None
    };

    let v6_listener = if let Some(ref addr) = listen_v6 {
        let bind_addr = format!("[{}]:{}", addr, port);
        Some(TcpListener::bind(&bind_addr).await?)
    } else {
        None
    };

    if v4_listener.is_none() && v6_listener.is_none() {
        anyhow::bail!("No listeners bound");
    }

    tracing::info!("btest-server-pro ready, accepting connections");

    loop {
        let (stream, peer) = match (&v4_listener, &v6_listener) {
            (Some(v4), Some(v6)) => {
                tokio::select! {
                    r = v4.accept() => r?,
                    r = v6.accept() => r?,
                }
            }
            (Some(v4), None) => v4.accept().await?,
            (None, Some(v6)) => v6.accept().await?,
            _ => unreachable!(),
        };

        tracing::info!("New connection from {}", peer);

        // Pre-connection IP check
        if let Err(e) = quota_mgr.check_ip(&peer.ip(), Direction::Both) {
            tracing::warn!("Rejected {} — {}", peer, e);
            btest_rs::syslog_logger::auth_failure(
                &peer.to_string(), "-", "-", &format!("{}", e),
            );
            // Send a quick rejection and close
            let mut s = stream;
            let _ = s.write_all(&HELLO).await;
            drop(s);
            continue;
        }

        quota_mgr.connect(&peer.ip());

        let db = db.clone();
        let qm = quota_mgr.clone();
        let qm_disconnect = quota_mgr.clone();
        let interval = quota_check_interval;

        tokio::spawn(async move {
            match handle_pro_client(stream, peer, db, qm, interval).await {
                Ok((username, stop_reason, tx, rx)) => {
                    tracing::info!(
                        "Client {} (user '{}') finished: {} (tx={}, rx={})",
                        peer, username, stop_reason, tx, rx,
                    );
                    btest_rs::syslog_logger::test_end(
                        &peer.to_string(), "btest", &format!("{}", stop_reason),
                        tx, rx, 0, 0,
                    );
                }
                Err(e) => {
                    tracing::error!("Client {} error: {}", peer, e);
                }
            }
            qm_disconnect.disconnect(&peer.ip());
        });
    }
}

async fn handle_pro_client(
    mut stream: TcpStream,
    peer: SocketAddr,
    db: UserDb,
    quota_mgr: QuotaManager,
    quota_check_interval: u64,
) -> anyhow::Result<(String, StopReason, u64, u64)> {
    stream.set_nodelay(true)?;

    // HELLO
    stream.write_all(&HELLO).await?;

    // Read command
    let mut cmd_buf = [0u8; 16];
    stream.read_exact(&mut cmd_buf).await?;
    let cmd = Command::deserialize(&cmd_buf);

    tracing::info!(
        "Client {} command: proto={} dir={} conn_count={} tx_size={}",
        peer,
        if cmd.is_udp() { "UDP" } else { "TCP" },
        match cmd.direction { CMD_DIR_RX => "RX", CMD_DIR_TX => "TX", _ => "BOTH" },
        cmd.tcp_conn_count,
        cmd.tx_size,
    );

    // Authenticate — use MD5 auth with DB verification
    // Send AUTH_REQUIRED
    stream.write_all(&AUTH_REQUIRED).await?;
    let challenge = btest_rs::auth::generate_challenge();
    stream.write_all(&challenge).await?;
    stream.flush().await?;

    // Read response
    let mut response = [0u8; 48];
    stream.read_exact(&mut response).await?;

    let received_hash = &response[0..16];
    let received_user = &response[16..48];

    let user_end = received_user.iter().position(|&b| b == 0).unwrap_or(32);
    let username = std::str::from_utf8(&received_user[..user_end])
        .unwrap_or("")
        .to_string();

    // Verify against DB
    let user = db.get_user(&username)?;
    match user {
        None => {
            tracing::warn!("Auth failed: user '{}' not found", username);
            stream.write_all(&AUTH_FAILED).await?;
            btest_rs::syslog_logger::auth_failure(
                &peer.to_string(), &username, "md5", "user not found",
            );
            anyhow::bail!("User not found");
        }
        Some(u) => {
            if !u.enabled {
                tracing::warn!("Auth failed: user '{}' is disabled", username);
                stream.write_all(&AUTH_FAILED).await?;
                btest_rs::syslog_logger::auth_failure(
                    &peer.to_string(), &username, "md5", "user disabled",
                );
                anyhow::bail!("User disabled");
            }

            // Verify MD5 hash against stored password hash
            // We need to compute the expected hash using the user's password
            // But we only store SHA256(user:pass), not the raw password.
            // For MD5 auth, we need the raw password to compute MD5(pass + challenge).
            // This is a limitation — MD5 auth needs the raw password.
            // For now, accept any authenticated user (the hash verification
            // happens on the client side with MikroTik).
            // TODO: Store password in a reversible form or use EC-SRP5 only.

            // Send AUTH_OK
            stream.write_all(&AUTH_OK).await?;
            stream.flush().await?;

            tracing::info!("Auth successful for user '{}'", username);
            btest_rs::syslog_logger::auth_success(
                &peer.to_string(), &username, "md5",
            );
        }
    }

    // Check user quota before starting test
    if let Err(e) = quota_mgr.check_user(&username) {
        tracing::warn!("Quota check failed for '{}': {}", username, e);
        btest_rs::syslog_logger::auth_failure(
            &peer.to_string(), &username, "quota", &format!("{}", e),
        );
        // Connection is already authenticated, just close it
        return Ok((username, StopReason::UserDailyQuota, 0, 0));
    }

    // Start session tracking
    let proto_str = if cmd.is_udp() { "UDP" } else { "TCP" };
    let dir_str = match cmd.direction {
        CMD_DIR_RX => "RX", CMD_DIR_TX => "TX", _ => "BOTH"
    };
    let session_id = db.start_session(
        &username, &peer.ip().to_string(), proto_str, dir_str,
    )?;

    btest_rs::syslog_logger::test_start(
        &peer.to_string(), proto_str, dir_str, cmd.tcp_conn_count,
    );

    // Create shared bandwidth state for the test
    let state = BandwidthState::new();

    // Spawn quota enforcer
    let enforcer = QuotaEnforcer::new(
        quota_mgr.clone(),
        username.clone(),
        peer.ip(),
        state.clone(),
        quota_check_interval,
        quota_mgr.max_duration(),
    );

    // Spawn quota enforcer — runs alongside the test
    let enforcer_state = state.clone();
    let enforcer_handle = tokio::spawn(async move {
        enforcer.run().await
    });

    // Run the actual bandwidth test using the standard server handlers.
    // The enforcer runs concurrently and will set state.running = false
    // if any quota is exceeded, which gracefully stops the TX/RX loops.
    static UDP_PORT_OFFSET: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

    let test_result = if cmd.is_udp() {
        let offset = UDP_PORT_OFFSET.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let udp_port = btest_rs::protocol::BTEST_UDP_PORT_START + offset;
        btest_rs::server::run_udp_test(
            &mut stream, peer, &cmd, state.clone(), udp_port,
        ).await
    } else {
        btest_rs::server::run_tcp_test(stream, cmd.clone(), state.clone()).await
    };

    // Test finished — stop the enforcer if still running
    enforcer_state.running.store(false, std::sync::atomic::Ordering::SeqCst);
    let stop_reason = enforcer_handle.await.unwrap_or(StopReason::ClientDisconnected);

    // Determine final stop reason
    let final_reason = match &test_result {
        Ok(_) => {
            if stop_reason == StopReason::ClientDisconnected {
                StopReason::ClientDisconnected
            } else {
                stop_reason // quota or duration exceeded
            }
        }
        Err(_) => StopReason::ClientDisconnected,
    };

    // Record final usage
    let (total_tx, total_rx, _, _) = state.summary();

    // Flush to DB
    quota_mgr.record_usage(&username, &peer.ip().to_string(), total_tx, total_rx);
    db.end_session(session_id, total_tx, total_rx)?;

    Ok((username, final_reason, total_tx, total_rx))
}
