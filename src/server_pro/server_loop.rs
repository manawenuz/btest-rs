//! Enhanced server loop with quota enforcement.
//!
//! Wraps the standard btest server connection handler with:
//! - Pre-connection IP/user quota checks
//! - MD5 challenge-response auth against user DB
//! - TCP multi-connection session support
//! - Mid-session quota enforcement via QuotaEnforcer
//! - Post-session usage recording

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use btest_rs::protocol::*;
use btest_rs::bandwidth::BandwidthState;

use super::enforcer::{QuotaEnforcer, StopReason};
use super::quota::{Direction, QuotaManager};
use super::user_db::UserDb;

/// Pending TCP multi-connection session.
struct TcpSession {
    peer_ip: std::net::IpAddr,
    username: String,
    cmd: Command,
    streams: Vec<TcpStream>,
    expected: u8,
}

type SessionMap = Arc<Mutex<HashMap<u16, TcpSession>>>;

/// Run the pro server with quota enforcement.
pub async fn run_pro_server(
    port: u16,
    _ecsrp5: bool,
    listen_v4: Option<String>,
    listen_v6: Option<String>,
    db: UserDb,
    quota_mgr: QuotaManager,
    quota_check_interval: u64,
) -> anyhow::Result<()> {
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

    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));

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

        let db = db.clone();
        let qm = quota_mgr.clone();
        let interval = quota_check_interval;
        let sess = sessions.clone();

        tokio::spawn(async move {
            let is_primary = match handle_pro_connection(stream, peer, db, qm.clone(), interval, sess).await {
                Ok(Some((username, stop_reason, tx, rx))) => {
                    tracing::info!(
                        "Client {} (user '{}') finished: {} (tx={}, rx={})",
                        peer, username, stop_reason, tx, rx,
                    );
                    btest_rs::syslog_logger::test_end(
                        &peer.to_string(), "btest", &format!("{}", stop_reason),
                        tx, rx, 0, 0,
                    );
                    true
                }
                Ok(None) => false, // secondary connection or pending multi-conn
                Err(e) => {
                    tracing::error!("Client {} error: {}", peer, e);
                    true
                }
            };
            // Only decrement connection count for primary connections
            if is_primary {
                qm.disconnect(&peer.ip());
            }
        });
    }
}

/// Handle a single TCP connection. Returns None for secondary multi-conn joins.
async fn handle_pro_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    db: UserDb,
    quota_mgr: QuotaManager,
    quota_check_interval: u64,
    sessions: SessionMap,
) -> anyhow::Result<Option<(String, StopReason, u64, u64)>> {
    stream.set_nodelay(true)?;

    // HELLO
    stream.write_all(&HELLO).await?;

    // Read command (or session token for secondary connections)
    let mut cmd_buf = [0u8; 16];
    stream.read_exact(&mut cmd_buf).await?;

    // Check if this is a secondary connection joining an existing TCP session
    // Secondary connections send [HI, LO, ...] matching an existing session token
    {
        let potential_token = u16::from_be_bytes([cmd_buf[0], cmd_buf[1]]);
        let mut map = sessions.lock().await;
        if let Some(session) = map.get_mut(&potential_token) {
            if session.peer_ip == peer.ip()
                && session.streams.len() < session.expected as usize
            {
                tracing::info!(
                    "Secondary connection from {} joining session (token={:04x}, {}/{})",
                    peer, potential_token,
                    session.streams.len() + 1, session.expected,
                );

                // Auth the secondary connection with same token response
                let ok = [0x01, cmd_buf[0], cmd_buf[1], 0x00];
                stream.write_all(&ok).await?;
                stream.flush().await?;

                session.streams.push(stream);

                // If all connections have joined, start the test
                if session.streams.len() >= session.expected as usize {
                    let session = map.remove(&potential_token).unwrap();
                    let db2 = db.clone();
                    let qm2 = quota_mgr.clone();
                    tokio::spawn(async move {
                        match run_pro_multiconn_test(
                            session.streams, session.cmd, peer,
                            &session.username, db2, qm2, quota_check_interval,
                        ).await {
                            Ok((stop, tx, rx)) => {
                                tracing::info!(
                                    "Multi-conn {} (user '{}') finished: {} (tx={}, rx={})",
                                    peer, session.username, stop, tx, rx,
                                );
                            }
                            Err(e) => {
                                tracing::error!("Multi-conn {} error: {}", peer, e);
                            }
                        }
                    });
                }

                return Ok(None);
            }
        }
    }

    // Primary connection — check IP quota/connection limit now
    if let Err(e) = quota_mgr.check_ip(&peer.ip(), Direction::Both) {
        tracing::warn!("Rejected {} — {}", peer, e);
        btest_rs::syslog_logger::auth_failure(
            &peer.to_string(), "-", "-", &format!("{}", e),
        );
        return Ok(None);
    }
    quota_mgr.connect(&peer.ip());

    let cmd = Command::deserialize(&cmd_buf);

    tracing::info!(
        "Client {} command: proto={} dir={} conn_count={} tx_size={}",
        peer,
        if cmd.is_udp() { "UDP" } else { "TCP" },
        match cmd.direction { CMD_DIR_RX => "RX", CMD_DIR_TX => "TX", _ => "BOTH" },
        cmd.tcp_conn_count,
        cmd.tx_size,
    );

    // Build auth OK response with session token for multi-connection
    let is_tcp_multi = !cmd.is_udp() && cmd.tcp_conn_count > 0;
    let session_token: u16 = if is_tcp_multi {
        rand::random::<u16>() | 0x0101 // ensure both bytes non-zero
    } else {
        0
    };
    let ok_response: [u8; 4] = if is_tcp_multi {
        [0x01, (session_token >> 8) as u8, (session_token & 0xFF) as u8, 0x00]
    } else {
        AUTH_OK
    };

    // Authenticate — MD5 challenge-response against DB
    stream.write_all(&AUTH_REQUIRED).await?;
    let challenge = btest_rs::auth::generate_challenge();
    stream.write_all(&challenge).await?;
    stream.flush().await?;

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

            // Verify MD5 hash against stored raw password
            if let Ok(Some(raw_pass)) = db.get_password(&username) {
                let expected_hash = btest_rs::auth::compute_auth_hash(&raw_pass, &challenge);
                if received_hash != expected_hash {
                    tracing::warn!("Auth failed: password mismatch for user '{}'", username);
                    stream.write_all(&AUTH_FAILED).await?;
                    btest_rs::syslog_logger::auth_failure(
                        &peer.to_string(), &username, "md5", "password mismatch",
                    );
                    anyhow::bail!("Auth failed");
                }
            }
            // If no raw password stored, accept (backwards compat with old DB entries)

            stream.write_all(&ok_response).await?;
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
        return Ok(Some((username, StopReason::UserDailyQuota, 0, 0)));
    }

    // TCP multi-connection: register session and wait for secondary connections
    if is_tcp_multi {
        tracing::info!(
            "TCP multi-connection: waiting for {} connections (token={:04x})",
            cmd.tcp_conn_count, session_token,
        );
        let mut map = sessions.lock().await;
        map.insert(session_token, TcpSession {
            peer_ip: peer.ip(),
            username: username.clone(),
            cmd: cmd.clone(),
            streams: vec![stream],
            expected: cmd.tcp_conn_count, // tcp_conn_count includes the primary
        });
        // The test will be started when all connections join (in the secondary handler above)
        return Ok(None);
    }

    // Single-connection test
    run_pro_single_test(stream, cmd, peer, &username, db, quota_mgr, quota_check_interval).await
        .map(|(stop, tx, rx)| Some((username, stop, tx, rx)))
}

/// Run a single-connection bandwidth test with quota enforcement.
async fn run_pro_single_test(
    stream: TcpStream,
    cmd: Command,
    peer: SocketAddr,
    username: &str,
    db: UserDb,
    quota_mgr: QuotaManager,
    quota_check_interval: u64,
) -> anyhow::Result<(StopReason, u64, u64)> {
    let proto_str = if cmd.is_udp() { "UDP" } else { "TCP" };
    let dir_str = match cmd.direction {
        CMD_DIR_RX => "RX", CMD_DIR_TX => "TX", _ => "BOTH"
    };
    let session_id = db.start_session(
        username, &peer.ip().to_string(), proto_str, dir_str,
    )?;

    btest_rs::syslog_logger::test_start(
        &peer.to_string(), proto_str, dir_str, cmd.tcp_conn_count,
    );

    let state = BandwidthState::new();

    // Set byte budget
    let budget = quota_mgr.remaining_budget(username, &peer.ip());
    if budget < u64::MAX {
        state.set_budget(budget);
        tracing::info!("Byte budget for '{}' from {}: {} bytes", username, peer.ip(), budget);
    }

    let enforcer = QuotaEnforcer::new(
        quota_mgr.clone(),
        username.to_string(),
        peer.ip(),
        state.clone(),
        quota_check_interval,
        quota_mgr.max_duration(),
    );

    let enforcer_state = state.clone();
    let enforcer_handle = tokio::spawn(async move {
        enforcer.run().await
    });

    static UDP_PORT_OFFSET: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

    let mut stream_mut = stream;
    let test_result = if cmd.is_udp() {
        let offset = UDP_PORT_OFFSET.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let udp_port = btest_rs::protocol::BTEST_UDP_PORT_START + offset;
        btest_rs::server::run_udp_test(
            &mut stream_mut, peer, &cmd, state.clone(), udp_port,
        ).await
    } else {
        btest_rs::server::run_tcp_test(stream_mut, cmd.clone(), state.clone()).await
    };

    enforcer_state.running.store(false, std::sync::atomic::Ordering::SeqCst);
    let stop_reason = enforcer_handle.await.unwrap_or(StopReason::ClientDisconnected);

    let final_reason = match &test_result {
        Ok(_) => {
            if stop_reason == StopReason::ClientDisconnected {
                StopReason::ClientDisconnected
            } else {
                stop_reason
            }
        }
        Err(_) => StopReason::ClientDisconnected,
    };

    let (total_tx, total_rx, _, _) = state.summary();
    quota_mgr.record_usage(username, &peer.ip().to_string(), total_tx, total_rx);
    db.end_session(session_id, total_tx, total_rx)?;

    Ok((final_reason, total_tx, total_rx))
}

/// Run a TCP multi-connection test with all streams collected.
/// Delegates to the standard multi-conn handler which correctly manages
/// TX+status injection for bidirectional mode.
async fn run_pro_multiconn_test(
    streams: Vec<TcpStream>,
    cmd: Command,
    peer: SocketAddr,
    username: &str,
    db: UserDb,
    quota_mgr: QuotaManager,
    quota_check_interval: u64,
) -> anyhow::Result<(StopReason, u64, u64)> {
    let dir_str = match cmd.direction {
        CMD_DIR_RX => "RX", CMD_DIR_TX => "TX", _ => "BOTH"
    };
    let session_id = db.start_session(
        username, &peer.ip().to_string(), "TCP", dir_str,
    )?;

    tracing::info!(
        "Starting TCP multi-conn test: {} streams, dir={}",
        streams.len(), dir_str,
    );

    let state = BandwidthState::new();

    let budget = quota_mgr.remaining_budget(username, &peer.ip());
    if budget < u64::MAX {
        state.set_budget(budget);
    }

    let enforcer = QuotaEnforcer::new(
        quota_mgr.clone(),
        username.to_string(),
        peer.ip(),
        state.clone(),
        quota_check_interval,
        quota_mgr.max_duration(),
    );

    let enforcer_state = state.clone();
    let enforcer_handle = tokio::spawn(async move {
        enforcer.run().await
    });

    // Use the standard multi-connection handler which correctly handles
    // all direction modes (TX, RX, BOTH with status injection)
    let _test_result = btest_rs::server::run_tcp_multiconn_test(
        streams, cmd, state.clone(),
    ).await;

    enforcer_state.running.store(false, std::sync::atomic::Ordering::SeqCst);
    let stop_reason = enforcer_handle.await.unwrap_or(StopReason::ClientDisconnected);

    let (total_tx, total_rx, _, _) = state.summary();
    quota_mgr.record_usage(username, &peer.ip().to_string(), total_tx, total_rx);
    db.end_session(session_id, total_tx, total_rx)?;

    Ok((stop_reason, total_tx, total_rx))
}
