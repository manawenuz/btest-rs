use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

use crate::auth;
use crate::bandwidth::{self, BandwidthState};
use crate::protocol::*;

pub async fn run_server(
    port: u16,
    auth_user: Option<String>,
    auth_pass: Option<String>,
) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("btest server listening on {}", addr);

    let udp_port_offset = Arc::new(std::sync::atomic::AtomicU16::new(0));

    loop {
        let (stream, peer) = listener.accept().await?;
        tracing::info!("New connection from {}", peer);

        let auth_user = auth_user.clone();
        let auth_pass = auth_pass.clone();
        let udp_offset = udp_port_offset.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, peer, auth_user, auth_pass, udp_offset).await {
                tracing::error!("Client {} error: {}", peer, e);
            }
        });
    }
}

async fn handle_client(
    mut stream: TcpStream,
    peer: SocketAddr,
    auth_user: Option<String>,
    auth_pass: Option<String>,
    udp_port_offset: Arc<std::sync::atomic::AtomicU16>,
) -> Result<()> {
    stream.set_nodelay(true)?;

    send_hello(&mut stream).await?;

    let cmd = recv_command(&mut stream).await?;
    tracing::info!(
        "Client {} command: proto={} dir={} tx_size={} remote_speed={} local_speed={}",
        peer,
        if cmd.is_udp() { "UDP" } else { "TCP" },
        match cmd.direction {
            CMD_DIR_RX => "RX",
            CMD_DIR_TX => "TX",
            CMD_DIR_BOTH => "BOTH",
            _ => "?",
        },
        cmd.tx_size,
        cmd.remote_tx_speed,
        cmd.local_tx_speed,
    );

    auth::server_authenticate(
        &mut stream,
        auth_user.as_deref(),
        auth_pass.as_deref(),
    )
    .await?;

    if cmd.is_udp() {
        run_udp_test_server(&mut stream, peer, &cmd, udp_port_offset).await
    } else {
        run_tcp_test_server(stream, cmd).await
    }
}

// --- TCP Test Server ---

async fn run_tcp_test_server(stream: TcpStream, cmd: Command) -> Result<()> {
    let state = BandwidthState::new();
    let tx_size = cmd.tx_size as usize;
    let server_should_tx = cmd.server_tx();
    let server_should_rx = cmd.server_rx();
    let tx_speed = cmd.remote_tx_speed;

    let (reader, writer) = stream.into_split();

    // IMPORTANT: Do NOT drop unused halves - dropping sends TCP FIN
    let mut _writer_keepalive = None;
    let mut _reader_keepalive = None;

    let state_tx = state.clone();
    let tx_handle = if server_should_tx {
        Some(tokio::spawn(async move {
            tcp_tx_loop(writer, tx_size, tx_speed, state_tx).await
        }))
    } else {
        _writer_keepalive = Some(writer);
        None
    };

    let state_rx = state.clone();
    let rx_handle = if server_should_rx {
        Some(tokio::spawn(async move {
            tcp_rx_loop(reader, state_rx).await
        }))
    } else {
        _reader_keepalive = Some(reader);
        None
    };

    status_report_loop(&cmd, &state).await;

    state.running.store(false, Ordering::SeqCst);
    if let Some(h) = tx_handle { let _ = h.await; }
    if let Some(h) = rx_handle { let _ = h.await; }
    Ok(())
}

async fn tcp_tx_loop(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    tx_size: usize,
    tx_speed: u32,
    state: Arc<BandwidthState>,
) {
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut packet = vec![0u8; tx_size];
    packet[0] = STATUS_MSG_TYPE;
    let mut interval = bandwidth::calc_send_interval(tx_speed, tx_size as u16);
    let mut next_send = Instant::now();

    while state.running.load(Ordering::Relaxed) {
        if writer.write_all(&packet).await.is_err() {
            break;
        }
        state.tx_bytes.fetch_add(tx_size as u64, Ordering::Relaxed);

        if state.tx_speed_changed.load(Ordering::Relaxed) {
            state.tx_speed_changed.store(false, Ordering::Relaxed);
            let new_speed = state.tx_speed.load(Ordering::Relaxed);
            interval = bandwidth::calc_send_interval(new_speed, tx_size as u16);
            next_send = Instant::now();
        }

        match interval {
            Some(iv) => {
                next_send += iv;
                let now = Instant::now();
                if next_send > now {
                    tokio::time::sleep(next_send - now).await;
                }
            }
            None => {
                tokio::task::yield_now().await;
            }
        }
    }
}

async fn tcp_rx_loop(mut reader: tokio::net::tcp::OwnedReadHalf, state: Arc<BandwidthState>) {
    let mut buf = vec![0u8; 65536];
    while state.running.load(Ordering::Relaxed) {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                state.rx_bytes.fetch_add(n as u64, Ordering::Relaxed);
            }
        }
    }
}

// --- UDP Test Server ---

async fn run_udp_test_server(
    stream: &mut TcpStream,
    peer: SocketAddr,
    cmd: &Command,
    udp_port_offset: Arc<std::sync::atomic::AtomicU16>,
) -> Result<()> {
    let offset = udp_port_offset.fetch_add(1, Ordering::SeqCst);
    let server_udp_port = BTEST_UDP_PORT_START + offset;
    let client_udp_port = server_udp_port + BTEST_PORT_CLIENT_OFFSET;

    stream.write_all(&server_udp_port.to_be_bytes()).await?;
    stream.flush().await?;

    tracing::info!(
        "UDP test: server_port={}, client_port={}, peer={}",
        server_udp_port, client_udp_port, peer,
    );

    let udp = UdpSocket::bind(format!("0.0.0.0:{}", server_udp_port)).await?;
    let client_udp_addr: SocketAddr =
        format!("{}:{}", peer.ip(), client_udp_port).parse().unwrap();
    udp.connect(client_udp_addr).await?;

    let state = BandwidthState::new();
    let tx_size = cmd.tx_size as usize;
    let server_should_tx = cmd.server_tx();
    let server_should_rx = cmd.server_rx();
    let tx_speed = cmd.remote_tx_speed;

    let udp = Arc::new(udp);

    let state_tx = state.clone();
    let udp_tx = udp.clone();
    let tx_handle = if server_should_tx {
        Some(tokio::spawn(async move {
            udp_tx_loop(&udp_tx, tx_size, tx_speed, state_tx).await
        }))
    } else {
        None
    };

    let state_rx = state.clone();
    let udp_rx = udp.clone();
    let rx_handle = if server_should_rx {
        Some(tokio::spawn(async move {
            udp_rx_loop(&udp_rx, state_rx).await
        }))
    } else {
        None
    };

    // Status exchange using select! to match C pselect() behavior
    udp_status_loop(stream, cmd, &state).await;

    state.running.store(false, Ordering::SeqCst);
    if let Some(h) = tx_handle { let _ = h.await; }
    if let Some(h) = rx_handle { let _ = h.await; }
    Ok(())
}

async fn udp_tx_loop(
    socket: &UdpSocket,
    tx_size: usize,
    initial_tx_speed: u32,
    state: Arc<BandwidthState>,
) {
    let mut seq: u32 = 0;
    let mut packet = vec![0u8; tx_size];
    let mut interval = bandwidth::calc_send_interval(initial_tx_speed, tx_size as u16);
    let mut next_send = Instant::now();
    let mut consecutive_errors: u32 = 0;

    while state.running.load(Ordering::Relaxed) {
        packet[0..4].copy_from_slice(&seq.to_be_bytes());

        match socket.send(&packet).await {
            Ok(n) => {
                seq = seq.wrapping_add(1);
                state.tx_bytes.fetch_add(n as u64, Ordering::Relaxed);
                consecutive_errors = 0;
            }
            Err(_) => {
                consecutive_errors += 1;
                if consecutive_errors > 1000 {
                    tracing::warn!("UDP TX: too many consecutive send errors, stopping");
                    break;
                }
                // Back off on ENOBUFS/EAGAIN
                tokio::time::sleep(Duration::from_micros(200)).await;
                continue;
            }
        }

        // Pick up dynamic speed changes from status loop
        if state.tx_speed_changed.load(Ordering::Relaxed) {
            state.tx_speed_changed.store(false, Ordering::Relaxed);
            let new_speed = state.tx_speed.load(Ordering::Relaxed);
            interval = bandwidth::calc_send_interval(new_speed, tx_size as u16);
            next_send = Instant::now();
            tracing::debug!("TX speed adjusted to {} bps ({:.2} Mbps)",
                new_speed, new_speed as f64 / 1_000_000.0);
        }

        match interval {
            Some(iv) => {
                next_send += iv;
                let now = Instant::now();
                if next_send > now {
                    tokio::time::sleep(next_send - now).await;
                }
            }
            None => {
                // Unlimited: yield every 64 packets to keep system responsive
                if seq % 64 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        }
    }
}

async fn udp_rx_loop(socket: &UdpSocket, state: Arc<BandwidthState>) {
    let mut buf = vec![0u8; 65536];
    let mut last_seq: Option<u32> = None;

    while state.running.load(Ordering::Relaxed) {
        match tokio::time::timeout(Duration::from_secs(5), socket.recv(&mut buf)).await {
            Ok(Ok(n)) if n >= 4 => {
                state.rx_bytes.fetch_add(n as u64, Ordering::Relaxed);
                state.rx_packets.fetch_add(1, Ordering::Relaxed);

                let seq = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
                if let Some(last) = last_seq {
                    let expected = last.wrapping_add(1);
                    if seq > expected {
                        let lost = seq - expected;
                        state.rx_lost_packets.fetch_add(lost as u64, Ordering::Relaxed);
                    }
                }
                last_seq = Some(seq);
                state.last_udp_seq.store(seq, Ordering::Relaxed);
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                tracing::debug!("UDP recv error: {}", e);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(_) => {
                tracing::debug!("UDP RX timeout");
            }
        }
    }
}

// --- Status Reporting ---

async fn status_report_loop(cmd: &Command, state: &BandwidthState) {
    let mut seq: u32 = 0;
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;
        if !state.running.load(Ordering::Relaxed) {
            break;
        }

        seq += 1;

        if cmd.server_tx() {
            let tx = state.tx_bytes.swap(0, Ordering::Relaxed);
            bandwidth::print_status(seq, "TX", tx, Duration::from_secs(1), None);
        }

        if cmd.server_rx() {
            let rx = state.rx_bytes.swap(0, Ordering::Relaxed);
            let lost = state.rx_lost_packets.swap(0, Ordering::Relaxed);
            let lost_opt = if cmd.is_udp() { Some(lost) } else { None };
            bandwidth::print_status(seq, "RX", rx, Duration::from_secs(1), lost_opt);
        }
    }
}

/// UDP status exchange loop - matches C pselect() behavior exactly:
///   1. Wait up to 1 second for client status (like pselect with 1s timeout)
///   2. If client sent status, read and process it
///   3. ALWAYS send our status (unconditional, matching C line 1048)
///   4. Reset counters and print stats
/// This sequential approach prevents the ticker from being starved.
async fn udp_status_loop(
    stream: &mut TcpStream,
    cmd: &Command,
    state: &BandwidthState,
) {
    let mut seq: u32 = 0;
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut status_buf = [0u8; STATUS_MSG_SIZE];
    let mut next_status = Instant::now() + Duration::from_secs(1);

    loop {
        if !state.running.load(Ordering::Relaxed) {
            break;
        }

        // Step 1: Wait for client status OR timeout (like C pselect)
        let now = Instant::now();
        let wait_time = if next_status > now {
            next_status - now
        } else {
            Duration::ZERO
        };

        // Try to read client's status within the remaining time window
        match tokio::time::timeout(wait_time, reader.read_exact(&mut status_buf)).await {
            Ok(Ok(_)) => {
                let client_status = StatusMessage::deserialize(&status_buf);
                tracing::debug!(
                    "RECV status: raw={:02x?} seq={} bytes_received={}",
                    &status_buf, client_status.seq, client_status.bytes_received,
                );

                if client_status.bytes_received > 0 && cmd.server_tx() {
                    let new_speed =
                        ((client_status.bytes_received as u64 * 8 * 3) / 2) as u32;
                    state.tx_speed.store(new_speed, Ordering::Relaxed);
                    state.tx_speed_changed.store(true, Ordering::Relaxed);
                    tracing::debug!(
                        "Speed adjust: client got {} bytes → our TX {:.2} Mbps",
                        client_status.bytes_received,
                        new_speed as f64 / 1_000_000.0,
                    );
                }

                if Instant::now() < next_status {
                    continue;
                }
            }
            Ok(Err(e)) => {
                tracing::debug!("Client TCP read error: {}", e);
                state.running.store(false, Ordering::SeqCst);
                break;
            }
            Err(_) => {
                // Timeout - 1 second elapsed
            }
        }

        // Step 2: ALWAYS send our status every 1 second
        seq += 1;
        next_status = Instant::now() + Duration::from_secs(1);

        let rx_bytes = state.rx_bytes.swap(0, Ordering::Relaxed);
        let tx_bytes = state.tx_bytes.swap(0, Ordering::Relaxed);
        let lost = state.rx_lost_packets.swap(0, Ordering::Relaxed);

        let status = StatusMessage {
            seq,
            bytes_received: rx_bytes as u32,
        };
        let serialized = status.serialize();
        tracing::debug!(
            "SEND status: raw={:02x?} seq={} bytes_received={} ({:.2} Mbps)",
            &serialized, seq, rx_bytes, rx_bytes as f64 * 8.0 / 1_000_000.0,
        );
        if writer.write_all(&serialized).await.is_err() {
            state.running.store(false, Ordering::SeqCst);
            break;
        }
        let _ = writer.flush().await;

        // Print local stats
        if cmd.server_tx() {
            bandwidth::print_status(seq, "TX", tx_bytes, Duration::from_secs(1), None);
        }
        if cmd.server_rx() {
            bandwidth::print_status(seq, "RX", rx_bytes, Duration::from_secs(1), Some(lost));
        }
    }
}
