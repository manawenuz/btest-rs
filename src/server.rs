use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;

use crate::auth;
use crate::bandwidth::{self, BandwidthState};
use crate::protocol::*;

/// Pending TCP multi-connection session: first connection creates this,
/// subsequent connections join via the session token.
struct TcpSession {
    peer_ip: std::net::IpAddr,
    streams: Vec<TcpStream>,
    expected: u8,
}

type SessionMap = Arc<Mutex<HashMap<u16, TcpSession>>>;

pub async fn run_server(
    port: u16,
    auth_user: Option<String>,
    auth_pass: Option<String>,
    use_ecsrp5: bool,
    listen_v4: Option<String>,
    listen_v6: Option<String>,
) -> Result<()> {
    // Pre-derive EC-SRP5 credentials if enabled
    let ecsrp5_creds = if use_ecsrp5 {
        match (auth_user.as_deref(), auth_pass.as_deref()) {
            (Some(user), Some(pass)) => {
                tracing::info!("EC-SRP5 authentication enabled for user '{}'", user);
                Some(Arc::new(crate::ecsrp5::EcSrp5Credentials::derive(user, pass)))
            }
            _ => {
                tracing::warn!("--ecsrp5 requires -a and -p to be set");
                None
            }
        }
    } else {
        None
    };

    let udp_port_offset = Arc::new(std::sync::atomic::AtomicU16::new(0));
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));

    // Bind IPv4 listener
    let v4_listener = if let Some(ref addr) = listen_v4 {
        let bind_addr = format!("{}:{}", addr, port);
        match TcpListener::bind(&bind_addr).await {
            Ok(l) => {
                tracing::info!("Listening on {} (IPv4)", bind_addr);
                Some(l)
            }
            Err(e) => {
                tracing::error!("Failed to bind {}: {}", bind_addr, e);
                None
            }
        }
    } else {
        None
    };

    // Bind IPv6 listener
    let v6_listener = if let Some(ref addr) = listen_v6 {
        let bind_addr = format!("[{}]:{}", addr, port);
        match TcpListener::bind(&bind_addr).await {
            Ok(l) => {
                tracing::info!("Listening on {} (IPv6)", bind_addr);
                Some(l)
            }
            Err(e) => {
                tracing::error!("Failed to bind {}: {}", bind_addr, e);
                None
            }
        }
    } else {
        None
    };

    if v4_listener.is_none() && v6_listener.is_none() {
        return Err(crate::protocol::BtestError::Protocol(
            "No listeners bound. Check --listen and --listen6 addresses.".into(),
        ));
    }

    loop {
        // Accept from whichever listener has a connection ready
        let (stream, peer) = match (&v4_listener, &v6_listener) {
            (Some(v4), Some(v6)) => {
                tokio::select! {
                    r = v4.accept() => r?,
                    r = v6.accept() => r?,
                }
            }
            (Some(v4), None) => v4.accept().await?,
            (None, Some(v6)) => v6.accept().await?,
            (None, None) => unreachable!(),
        };
        tracing::info!("New connection from {}", peer);

        let auth_user = auth_user.clone();
        let auth_pass = auth_pass.clone();
        let udp_offset = udp_port_offset.clone();
        let sessions = sessions.clone();
        let ecsrp5 = ecsrp5_creds.clone();

        tokio::spawn(async move {
            if let Err(e) =
                handle_client(stream, peer, auth_user, auth_pass, udp_offset, sessions, ecsrp5).await
            {
                let err_str = format!("{}", e);
                tracing::error!("Client {} error: {}", peer, err_str);
                if err_str.contains("uth") {
                    crate::syslog_logger::auth_failure(&peer.to_string(), "-", "-", &err_str);
                }
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
    sessions: SessionMap,
    ecsrp5_creds: Option<Arc<crate::ecsrp5::EcSrp5Credentials>>,
) -> Result<()> {
    stream.set_nodelay(true)?;

    send_hello(&mut stream).await?;

    // Read 16-byte command (or whatever the client sends)
    let mut cmd_buf = [0u8; 16];
    stream.read_exact(&mut cmd_buf).await?;
    tracing::debug!("Raw command from {}: {:02x?}", peer, cmd_buf);

    // Check if this is a secondary TCP connection joining a session.
    // Secondary connections send the session token in bytes 0-1 of their "command":
    //   [TOKEN_HI, TOKEN_LO, 0x02, 0x00, ...]
    // They do NOT do auth — just send them AUTH_OK with the token and they join.
    {
        let mut map = sessions.lock().await;
        let received_token = ((cmd_buf[0] as u16) << 8) | (cmd_buf[1] as u16);
        if let Some(session) = map.get_mut(&received_token) {
            if session.peer_ip == peer.ip()
                && session.streams.len() < session.expected as usize
            {
                tracing::info!(
                    "Client {} is secondary TCP connection (token={:04x})",
                    peer, received_token,
                );

                // No auth for secondary connections — just send OK with token
                let ok = [0x01, cmd_buf[0], cmd_buf[1], 0x00];
                stream.write_all(&ok).await?;
                stream.flush().await?;

                session.streams.push(stream);
                tracing::info!(
                    "Secondary connection joined ({}/{})",
                    session.streams.len() + 1,
                    session.expected,
                );
                return Ok(());
            }
        }
        drop(map);
    }

    // Primary connection: parse the command normally
    let cmd = Command::deserialize(&cmd_buf);
    if cmd.proto > 1 || cmd.direction == 0 || cmd.direction > 3 {
        return Err(BtestError::InvalidCommand);
    }

    tracing::info!(
        "Client {} command: proto={} dir={} conn_count={} tx_size={} remote_speed={} local_speed={}",
        peer,
        if cmd.is_udp() { "UDP" } else { "TCP" },
        match cmd.direction {
            CMD_DIR_RX => "RX",
            CMD_DIR_TX => "TX",
            CMD_DIR_BOTH => "BOTH",
            _ => "?",
        },
        cmd.tcp_conn_count,
        cmd.tx_size,
        cmd.remote_tx_speed,
        cmd.local_tx_speed,
    );

    // Build auth OK response - include session token for TCP multi-connection
    let is_tcp_multi = !cmd.is_udp() && cmd.tcp_conn_count > 0;
    let session_token: u16 = if is_tcp_multi {
        rand::random::<u16>() | 0x0101 // ensure both bytes non-zero
    } else {
        0
    };
    let ok_response: [u8; 4] = if is_tcp_multi {
        // MikroTik expects 01:HI:LO:00 for multi-connection support
        [0x01, (session_token >> 8) as u8, (session_token & 0xFF) as u8, 0x00]
    } else {
        AUTH_OK
    };

    if is_tcp_multi {
        tracing::info!(
            "TCP multi-connection: conn_count={}, session_token={:04x}, ok_response={:02x?}",
            cmd.tcp_conn_count, session_token, ok_response,
        );
    }

    // Check if this is a secondary connection joining an existing TCP session
    if is_tcp_multi {
        let mut map = sessions.lock().await;
        for (_token, session) in map.iter_mut() {
            if session.peer_ip == peer.ip()
                && session.streams.len() < session.expected as usize
            {
                tracing::info!(
                    "Client {} joining TCP session ({}/{})",
                    peer,
                    session.streams.len() + 1,
                    session.expected,
                );
                drop(map);
                // Secondary connections also do auth with the same session token response
                auth::server_authenticate(
                    &mut stream,
                    auth_user.as_deref(),
                    auth_pass.as_deref(),
                    &ok_response,
                )
                .await?;
                let mut map = sessions.lock().await;
                for (_t, s) in map.iter_mut() {
                    if s.peer_ip == peer.ip() && s.streams.len() < s.expected as usize {
                        s.streams.push(stream);
                        return Ok(());
                    }
                }
                return Ok(());
            }
        }
        drop(map);
    }

    // Primary connection auth
    if let Some(ref creds) = ecsrp5_creds {
        // EC-SRP5 authentication
        let auth_resp: [u8; 4] = [0x03, 0x00, 0x00, 0x00];
        stream.write_all(&auth_resp).await?;
        stream.flush().await?;

        crate::ecsrp5::server_authenticate(
            &mut stream,
            auth_user.as_deref().unwrap_or("admin"),
            creds,
        )
        .await?;

        // Send auth OK (with session token if multi-conn)
        stream.write_all(&ok_response).await?;
        stream.flush().await?;
    } else {
        // MD5 or no auth
        auth::server_authenticate(
            &mut stream,
            auth_user.as_deref(),
            auth_pass.as_deref(),
            &ok_response,
        )
        .await?;
    }

    // Log auth success and test start
    let auth_type = if ecsrp5_creds.is_some() { "ecsrp5" } else if auth_user.is_some() { "md5" } else { "none" };
    let proto_str = if cmd.is_udp() { "UDP" } else { "TCP" };
    let dir_str = match cmd.direction { CMD_DIR_RX => "RX", CMD_DIR_TX => "TX", _ => "BOTH" };
    crate::syslog_logger::auth_success(&peer.to_string(), auth_user.as_deref().unwrap_or("-"), auth_type);
    crate::syslog_logger::test_start(&peer.to_string(), proto_str, dir_str, cmd.tcp_conn_count);

    let result = if cmd.is_udp() {
        run_udp_test_server(&mut stream, peer, &cmd, udp_port_offset).await
    } else if is_tcp_multi {
        let conn_count = cmd.tcp_conn_count;

        // Register session for secondary connections to find
        {
            let mut map = sessions.lock().await;
            map.insert(session_token, TcpSession {
                peer_ip: peer.ip(),
                streams: Vec::new(),
                expected: conn_count,
            });
        }

        // Wait for secondary connections
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let count = {
                let map = sessions.lock().await;
                map.get(&session_token)
                    .map(|s| s.streams.len())
                    .unwrap_or(0)
            };
            if count + 1 >= conn_count as usize {
                break;
            }
            if Instant::now() > deadline {
                tracing::warn!(
                    "Timeout waiting for TCP connections ({}/{}), proceeding",
                    count + 1,
                    conn_count,
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let extra_streams = {
            let mut map = sessions.lock().await;
            map.remove(&session_token)
                .map(|s| s.streams)
                .unwrap_or_default()
        };

        let mut all_streams = vec![stream];
        all_streams.extend(extra_streams);

        tracing::info!(
            "TCP multi-connection: starting with {} total streams",
            all_streams.len(),
        );

        run_tcp_multiconn_server(all_streams, cmd).await
    } else {
        run_tcp_test_server(stream, cmd).await
    };

    crate::syslog_logger::test_end(&peer.to_string(), proto_str, dir_str);
    result
}

// --- TCP Test Server ---

async fn run_tcp_test_server(stream: TcpStream, cmd: Command) -> Result<()> {
    let state = BandwidthState::new();
    let tx_size = cmd.tx_size as usize;
    let server_should_tx = cmd.server_tx();
    let server_should_rx = cmd.server_rx();
    let tx_speed = cmd.remote_tx_speed;

    let (reader, writer) = stream.into_split();

    let mut _writer_keepalive = None;
    let mut _reader_keepalive = None;

    let state_tx = state.clone();
    let tx_handle = if server_should_tx && server_should_rx {
        // BOTH mode: TX data + inject status messages for the RX direction
        Some(tokio::spawn(async move {
            tcp_tx_with_status(writer, tx_size, tx_speed, state_tx).await
        }))
    } else if server_should_tx {
        // TX only
        Some(tokio::spawn(async move {
            tcp_tx_loop(writer, tx_size, tx_speed, state_tx).await
        }))
    } else if server_should_rx {
        // RX only: use writer for status messages
        let st = state.clone();
        Some(tokio::spawn(async move {
            tcp_status_sender(writer, st).await
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

    if server_should_tx && !server_should_rx {
        // TX-only: normal status loop reports TX stats
        status_report_loop(&cmd, &state).await;
    } else if server_should_tx && server_should_rx {
        // BOTH: TX loop injects status + prints RX. Just report TX here.
        let mut seq: u32 = 0;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            if !state.running.load(Ordering::Relaxed) { break; }
            seq += 1;
            let tx = state.tx_bytes.swap(0, Ordering::Relaxed);
            bandwidth::print_status(seq, "TX", tx, Duration::from_secs(1), None);
        }
    } else {
        // RX-only: tcp_status_sender handles everything. Just wait.
        while state.running.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    state.running.store(false, Ordering::SeqCst);
    if let Some(h) = tx_handle { let _ = h.await; }
    if let Some(h) = rx_handle { let _ = h.await; }
    Ok(())
}

/// TCP multi-connection.
async fn run_tcp_multiconn_server(streams: Vec<TcpStream>, cmd: Command) -> Result<()> {
    let state = BandwidthState::new();
    let tx_size = cmd.tx_size as usize;
    let server_should_tx = cmd.server_tx();
    let server_should_rx = cmd.server_rx();
    let tx_speed = cmd.remote_tx_speed;

    let mut tx_handles = Vec::new();
    let mut rx_handles = Vec::new();
    let mut _writer_keepalives: Vec<tokio::net::tcp::OwnedWriteHalf> = Vec::new();
    let mut _reader_keepalives: Vec<tokio::net::tcp::OwnedReadHalf> = Vec::new();

    for tcp_stream in streams {
        let (reader, writer) = tcp_stream.into_split();

        if server_should_tx && server_should_rx {
            let st = state.clone();
            tx_handles.push(tokio::spawn(async move {
                tcp_tx_with_status(writer, tx_size, tx_speed, st).await
            }));
        } else if server_should_tx {
            let st = state.clone();
            tx_handles.push(tokio::spawn(async move {
                tcp_tx_loop(writer, tx_size, tx_speed, st).await
            }));
        } else if server_should_rx {
            let st = state.clone();
            tx_handles.push(tokio::spawn(async move {
                tcp_status_sender(writer, st).await
            }));
        } else {
            _writer_keepalives.push(writer);
        }

        if server_should_rx {
            let st = state.clone();
            rx_handles.push(tokio::spawn(async move {
                tcp_rx_loop(reader, st).await
            }));
        } else {
            _reader_keepalives.push(reader);
        }
    }

    tracing::info!(
        "TCP multi-conn: {} TX tasks, {} RX tasks",
        tx_handles.len(),
        rx_handles.len(),
    );

    status_report_loop(&cmd, &state).await;

    state.running.store(false, Ordering::SeqCst);
    for h in tx_handles { let _ = h.await; }
    for h in rx_handles { let _ = h.await; }
    tracing::info!("TCP multi-connection test ended");
    Ok(())
}

async fn tcp_tx_loop(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    tx_size: usize,
    tx_speed: u32,
    state: Arc<BandwidthState>,
) {
    tcp_tx_loop_inner(&mut writer, tx_size, tx_speed, &state, false).await;
}

/// TCP TX loop that also sends status messages when `send_status` is true.
/// Used in bidirectional mode where the writer handles both data and status.
async fn tcp_tx_with_status(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    tx_size: usize,
    tx_speed: u32,
    state: Arc<BandwidthState>,
) {
    tcp_tx_loop_inner(&mut writer, tx_size, tx_speed, &state, true).await;
}

async fn tcp_tx_loop_inner(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    tx_size: usize,
    tx_speed: u32,
    state: &Arc<BandwidthState>,
    send_status: bool,
) {
    tokio::time::sleep(Duration::from_millis(100)).await;

    let packet = vec![0u8; tx_size];
    let mut interval = bandwidth::calc_send_interval(tx_speed, tx_size as u16);
    let mut next_send = Instant::now();
    let mut next_status = Instant::now() + Duration::from_secs(1);
    let mut status_seq: u32 = 0;

    while state.running.load(Ordering::Relaxed) {
        // Inject status message every ~1 second if in bidirectional mode
        if send_status && Instant::now() >= next_status {
            status_seq += 1;
            let rx_bytes = state.rx_bytes.swap(0, Ordering::Relaxed);
            let status = StatusMessage {
                seq: status_seq,
                bytes_received: rx_bytes as u32,
            };
            if writer.write_all(&status.serialize()).await.is_err() {
                state.running.store(false, Ordering::SeqCst);
                break;
            }
            bandwidth::print_status(status_seq, "RX", rx_bytes, Duration::from_secs(1), None);
            next_status = Instant::now() + Duration::from_secs(1);
        }

        if writer.write_all(&packet).await.is_err() {
            state.running.store(false, Ordering::SeqCst);
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
            Ok(0) | Err(_) => {
                state.running.store(false, Ordering::SeqCst);
                break;
            }
            Ok(n) => {
                state.rx_bytes.fetch_add(n as u64, Ordering::Relaxed);
            }
        }
    }
}

/// Send periodic 12-byte status messages on the TCP connection.
/// Used when server is in RX mode — tells the client how many bytes we received.
/// Send periodic 12-byte status messages on the TCP connection AND print local stats.
/// Used when server is in RX-only mode. Replaces the normal status_report_loop
/// because it needs the writer and must own the rx_bytes swap.
async fn tcp_status_sender(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    state: Arc<BandwidthState>,
) {
    let mut seq: u32 = 0;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.tick().await;

    while state.running.load(Ordering::Relaxed) {
        interval.tick().await;
        if !state.running.load(Ordering::Relaxed) {
            break;
        }

        seq += 1;
        // Swap to get bytes received this interval (atomic reset)
        let rx_bytes = state.rx_bytes.swap(0, Ordering::Relaxed);

        let status = StatusMessage {
            seq,
            bytes_received: rx_bytes as u32,
        };

        if writer.write_all(&status.serialize()).await.is_err() {
            state.running.store(false, Ordering::SeqCst);
            break;
        }
        let _ = writer.flush().await;

        // Also print locally
        bandwidth::print_status(seq, "RX", rx_bytes, Duration::from_secs(1), None);
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

    // Bind UDP on the same address family as the peer
    let bind_addr: SocketAddr = if peer.is_ipv6() {
        format!("[::]:{}",  server_udp_port).parse().unwrap()
    } else {
        format!("0.0.0.0:{}", server_udp_port).parse().unwrap()
    };
    let udp = UdpSocket::bind(bind_addr).await?;
    let client_udp_addr = SocketAddr::new(peer.ip(), client_udp_port);

    // When connection_count > 1, MikroTik sends UDP from MULTIPLE source ports
    // (base_port, base_port+1, ..., base_port+N-1) all to our single server port.
    // A connect()'d UDP socket only accepts from the one connected address,
    // silently dropping packets from the other ports.
    // So: only connect() for single-connection mode (enables send() without addr).
    // For multi-connection, we leave the socket unconnected and use send_to()/recv_from().
    // Don't connect() UDP socket when:
    // - Multi-connection mode (MikroTik sends from multiple source ports)
    // - IPv6 (macOS connected IPv6 UDP sockets have receive issues)
    let use_unconnected = cmd.tcp_conn_count > 0 || peer.is_ipv6();
    if !use_unconnected {
        udp.connect(client_udp_addr).await?;
    }

    tracing::info!(
        "UDP mode: conn_count={}, socket={}",
        cmd.tcp_conn_count.max(1),
        if use_unconnected { "unconnected" } else { "connected" },
    );

    let state = BandwidthState::new();
    let tx_size = cmd.tx_size as usize;
    let server_should_tx = cmd.server_tx();
    let server_should_rx = cmd.server_rx();
    let tx_speed = cmd.remote_tx_speed;

    let udp = Arc::new(udp);

    let state_tx = state.clone();
    let udp_tx = udp.clone();
    let tx_target = client_udp_addr;
    let is_multi = use_unconnected;
    let tx_handle = if server_should_tx {
        Some(tokio::spawn(async move {
            udp_tx_loop(&udp_tx, tx_size, tx_speed, state_tx, is_multi, tx_target).await
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
    multi_conn: bool,
    target: SocketAddr,
) {
    let mut seq: u32 = 0;
    let mut packet = vec![0u8; tx_size];
    let mut interval = bandwidth::calc_send_interval(initial_tx_speed, tx_size as u16);
    let mut next_send = Instant::now();
    let mut consecutive_errors: u32 = 0;

    while state.running.load(Ordering::Relaxed) {
        packet[0..4].copy_from_slice(&seq.to_be_bytes());

        let result = if multi_conn {
            socket.send_to(&packet, target).await
        } else {
            socket.send(&packet).await
        };
        match result {
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
        // Use recv_from to accept packets from any source port
        // (multi-connection MikroTik sends from multiple ports)
        match tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buf)).await {
            Ok(Ok((n, _src))) if n >= 4 => {
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
