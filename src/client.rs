use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

use crate::auth;
use crate::bandwidth::{self, BandwidthState};
use crate::iface::IfaceSpec;
use crate::protocol::*;

// The `btest` binary calls `run_client_bound` and compiles this module itself,
// so without this attribute the binary crate warns that the wrapper is unused.
#[allow(dead_code)]
pub async fn run_client(
    host: &str,
    port: u16,
    direction: u8,
    use_udp: bool,
    tx_speed: u32,
    rx_speed: u32,
    auth_user: Option<String>,
    auth_pass: Option<String>,
    nat_mode: bool,
    shared_state: Arc<BandwidthState>,
) -> Result<()> {
    run_client_bound(
        host,
        port,
        direction,
        use_udp,
        tx_speed,
        rx_speed,
        auth_user,
        auth_pass,
        nat_mode,
        shared_state,
        None,
    )
    .await
}

/// Client entry point that pins TCP and UDP sockets to `interface`.
///
/// `interface` is an interface name (`eth0`, `en0`) or a local address
/// (`10.0.0.5`). `None` keeps the kernel's default route.
pub async fn run_client_bound(
    host: &str,
    port: u16,
    direction: u8,
    use_udp: bool,
    tx_speed: u32,
    rx_speed: u32,
    auth_user: Option<String>,
    auth_pass: Option<String>,
    nat_mode: bool,
    shared_state: Arc<BandwidthState>,
    interface: Option<&str>,
) -> Result<()> {
    let spec = match interface {
        Some(name) => Some(IfaceSpec::parse(name)?),
        None => None,
    };
    let mut stream = connect_control(host, port, spec.as_ref()).await?;

    recv_hello(&mut stream).await?;
    tracing::info!("Connected to server");

    let proto = if use_udp {
        CMD_PROTO_UDP
    } else {
        CMD_PROTO_TCP
    };
    let mut cmd = Command::new(proto, direction);
    cmd.local_tx_speed = tx_speed;
    cmd.remote_tx_speed = rx_speed;

    send_command(&mut stream, &cmd).await?;

    let resp = recv_response(&mut stream).await?;
    if resp == AUTH_OK {
        // No auth required
    } else if resp == AUTH_REQUIRED {
        // MD5 auth
        match (auth_user.as_deref(), auth_pass.as_deref()) {
            (Some(user), Some(pass)) => {
                auth::client_authenticate(&mut stream, resp, user, pass).await?;
            }
            _ => {
                return Err(BtestError::Protocol(
                    "Server requires authentication but no credentials provided (-a/-p)".into(),
                ));
            }
        }
    } else if resp == [0x03, 0x00, 0x00, 0x00] {
        // EC-SRP5 auth (RouterOS >= 6.43)
        match (auth_user.as_deref(), auth_pass.as_deref()) {
            (Some(user), Some(pass)) => {
                crate::ecsrp5::client_authenticate(&mut stream, user, pass).await?;
                // After EC-SRP5, server sends AUTH_OK
                let post_auth = recv_response(&mut stream).await?;
                if post_auth != AUTH_OK {
                    return Err(BtestError::Protocol(format!(
                        "Unexpected post-EC-SRP5 response: {:02x?}",
                        post_auth
                    )));
                }
            }
            _ => {
                return Err(BtestError::Protocol(
                    "Server requires EC-SRP5 authentication. Provide credentials with -a/-p".into(),
                ));
            }
        }
    } else {
        return Err(BtestError::Protocol(format!(
            "Unexpected server response: {:02x?}",
            resp
        )));
    }

    tracing::info!(
        "Starting {} {} test",
        if use_udp { "UDP" } else { "TCP" },
        match direction {
            CMD_DIR_RX => "upload (client TX)",
            CMD_DIR_TX => "download (client RX)",
            CMD_DIR_BOTH => "bidirectional",
            _ => "unknown",
        },
    );

    if use_udp {
        let peer_ip = stream.peer_addr()?.ip();
        let local_ip = if spec.is_some() {
            Some(stream.local_addr()?.ip())
        } else {
            None
        };
        run_udp_test_client(
            &mut stream,
            peer_ip,
            local_ip,
            spec.as_ref(),
            &cmd,
            nat_mode,
            shared_state,
        )
        .await
    } else {
        run_tcp_test_client(stream, cmd, shared_state).await
    }
}

fn host_with_port(host: &str, port: u16) -> String {
    if host.starts_with('[') && host.ends_with(']') {
        format!("{host}:{port}")
    } else if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn tune_tcp(stream: &TcpStream) {
    let sock_ref = socket2::SockRef::from(stream);
    let _ = sock_ref.set_send_buffer_size(4 * 1024 * 1024);
    let _ = sock_ref.set_recv_buffer_size(4 * 1024 * 1024);
}

async fn connect_control(host: &str, port: u16, spec: Option<&IfaceSpec>) -> Result<TcpStream> {
    let addr = host_with_port(host, port);
    let Some(spec) = spec else {
        tracing::info!("Connecting to {addr}...");
        let stream = TcpStream::connect(&addr).await?;
        stream.set_nodelay(true)?;
        tune_tcp(&stream);
        return Ok(stream);
    };

    let remotes: Vec<SocketAddr> = tokio::net::lookup_host(&addr).await?.collect();
    if remotes.is_empty() {
        return Err(BtestError::Protocol(format!("could not resolve {host}")));
    }
    let matched: Vec<SocketAddr> = remotes
        .iter()
        .copied()
        .filter(|remote| spec.supports(remote.is_ipv6()))
        .collect();
    if matched.is_empty() {
        let want_v6 = remotes.iter().all(|remote| remote.is_ipv6());
        let want_v4 = remotes.iter().all(|remote| remote.is_ipv4());
        if want_v4 || want_v6 {
            return Err(spec.local_ip_for(want_v6).unwrap_err());
        }
        return Err(BtestError::Protocol(format!(
            "interface {} has no address matching {host} (IPv4/IPv6)",
            spec.requested
        )));
    }

    let mut last_err: Option<BtestError> = None;
    for remote in matched {
        let local_ip = spec.local_ip_for(remote.is_ipv6())?;
        match connect_bound(remote, local_ip, spec).await {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                tracing::debug!("connect to {remote} from {local_ip} failed: {e}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| BtestError::Protocol(format!("could not connect to {host}"))))
}

async fn connect_bound(
    remote: SocketAddr,
    local_ip: IpAddr,
    spec: &IfaceSpec,
) -> Result<TcpStream> {
    let socket = if remote.is_ipv6() {
        tokio::net::TcpSocket::new_v6()?
    } else {
        tokio::net::TcpSocket::new_v4()?
    };
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        crate::iface::pin_fd(socket.as_raw_fd(), spec, remote.is_ipv6())?;
    }
    let local = SocketAddr::new(local_ip, 0);
    socket.bind(local)?;
    tracing::info!(
        "Connecting to {remote} from {local_ip} ({})",
        spec.requested
    );
    let stream = socket.connect(remote).await?;
    stream.set_nodelay(true)?;
    tune_tcp(&stream);
    Ok(stream)
}

// --- TCP Test Client ---

async fn run_tcp_test_client(
    stream: TcpStream,
    cmd: Command,
    state: Arc<BandwidthState>,
) -> Result<()> {
    let tx_size = cmd.tx_size as usize;
    let client_should_tx = cmd.client_tx();
    let client_should_rx = cmd.client_rx();
    let tx_speed = cmd.local_tx_speed;

    let (reader, writer) = stream.into_split();

    // IMPORTANT: Do NOT drop unused halves - dropping OwnedWriteHalf sends TCP FIN,
    // causing the peer to think we disconnected. Use Option to conditionally move.
    let mut _writer_keepalive = None;
    let mut _reader_keepalive = None;

    let state_tx = state.clone();
    let tx_handle = if client_should_tx {
        Some(tokio::spawn(async move {
            tcp_client_tx_loop(writer, tx_size, tx_speed, state_tx).await
        }))
    } else {
        _writer_keepalive = Some(writer);
        None
    };

    let state_rx = state.clone();
    let rx_handle = if client_should_rx {
        Some(tokio::spawn(async move {
            tcp_client_rx_loop(reader, state_rx).await
        }))
    } else if client_should_tx {
        // TX-only: still need to read the server's status messages to get remote CPU.
        // Don't count these bytes as RX data.
        Some(tokio::spawn(async move {
            tcp_client_status_reader(reader, state_rx).await
        }))
    } else {
        _reader_keepalive = Some(reader);
        None
    };

    client_status_loop(&cmd, &state).await;

    state.running.store(false, Ordering::SeqCst);
    if let Some(h) = tx_handle {
        let _ = h.await;
    }
    if let Some(h) = rx_handle {
        let _ = h.await;
    }
    Ok(())
}

async fn tcp_client_tx_loop(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    tx_size: usize,
    tx_speed: u32,
    state: Arc<BandwidthState>,
) {
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut interval = bandwidth::calc_send_interval(tx_speed, tx_size as u16);
    // Use larger writes when running unlimited to reduce syscall overhead
    let effective_size = if interval.is_none() {
        tx_size.max(256 * 1024)
    } else {
        tx_size
    };
    let packet = vec![0u8; effective_size]; // TCP data is all zeros
    let mut next_send = Instant::now();

    while state.running.load(Ordering::Relaxed) {
        if writer.write_all(&packet).await.is_err() {
            break;
        }
        state
            .tx_bytes
            .fetch_add(effective_size as u64, Ordering::Relaxed);

        if state.tx_speed_changed.load(Ordering::Relaxed) {
            state.tx_speed_changed.store(false, Ordering::Relaxed);
            let new_speed = state.tx_speed.load(Ordering::Relaxed);
            interval = bandwidth::calc_send_interval(new_speed, tx_size as u16);
            next_send = Instant::now();
        }

        match interval {
            Some(iv) => {
                let now = Instant::now();
                if let Some(delay) = bandwidth::advance_next_send(&mut next_send, iv, now) {
                    tokio::time::sleep(delay).await;
                }
            }
            None => {
                tokio::task::yield_now().await;
            }
        }
    }
}

async fn tcp_client_rx_loop(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    state: Arc<BandwidthState>,
) {
    let mut buf = vec![0u8; 256 * 1024];
    while state.running.load(Ordering::Relaxed) {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                state.rx_bytes.fetch_add(n as u64, Ordering::Relaxed);
                // Scan for interleaved 12-byte status messages from the server.
                // In BOTH mode, the server's TX loop injects status messages into the
                // data stream. Status starts with 0x07 (STATUS_MSG_TYPE) and byte 1
                // has the high bit set (0x80 | cpu%). Data packets are all zeros.
                if n >= STATUS_MSG_SIZE {
                    for i in 0..=(n - STATUS_MSG_SIZE) {
                        if buf[i] == STATUS_MSG_TYPE && buf[i + 1] >= 0x80 {
                            let cpu = buf[i + 1] & 0x7F;
                            state.remote_cpu.store(cpu.min(100), Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Read only status messages from the server (TX-only mode).
/// The server sends 12-byte status messages on the TCP connection even when
/// the client is only transmitting. We need to read them to get remote CPU
/// and to prevent the TCP receive buffer from filling up.
async fn tcp_client_status_reader(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    state: Arc<BandwidthState>,
) {
    let mut buf = [0u8; STATUS_MSG_SIZE];
    while state.running.load(Ordering::Relaxed) {
        match reader.read_exact(&mut buf).await {
            Ok(_) => {
                if buf[0] == STATUS_MSG_TYPE && buf[1] >= 0x80 {
                    let status = StatusMessage::deserialize(&buf);
                    state.remote_cpu.store(status.cpu_load, Ordering::Relaxed);
                    // Use server's bytes_received for TX speed adaptation
                    if status.bytes_received > 0 {
                        let new_speed = ((status.bytes_received as u64 * 8 * 3) / 2) as u32;
                        state.tx_speed.store(new_speed, Ordering::Relaxed);
                        state.tx_speed_changed.store(true, Ordering::Relaxed);
                    }
                }
            }
            Err(_) => break,
        }
    }
}

// --- UDP Test Client ---

async fn run_udp_test_client(
    stream: &mut TcpStream,
    peer_ip: IpAddr,
    local_ip: Option<IpAddr>,
    spec: Option<&IfaceSpec>,
    cmd: &Command,
    nat_mode: bool,
    state: Arc<BandwidthState>,
) -> Result<()> {
    let mut port_buf = [0u8; 2];
    stream.read_exact(&mut port_buf).await?;
    let server_udp_port = u16::from_be_bytes(port_buf);
    let client_udp_port = server_udp_port + BTEST_PORT_CLIENT_OFFSET;

    tracing::info!(
        "UDP ports: server={}, client={}",
        server_udp_port,
        client_udp_port,
    );

    let is_ipv6 = peer_ip.is_ipv6();
    let bind_ip = local_ip.unwrap_or(if is_ipv6 {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    });
    let bind_addr = SocketAddr::new(bind_ip, client_udp_port);
    let udp = bind_udp_socket(bind_addr, spec)?;
    let server_udp_addr = SocketAddr::new(peer_ip, server_udp_port);
    udp.connect(server_udp_addr).await?;

    if nat_mode {
        tracing::info!("NAT mode: sending probe packet");
        udp.send(&[]).await?;
    }

    let tx_size = cmd.tx_size as usize;
    let client_should_tx = cmd.client_tx();
    let client_should_rx = cmd.client_rx();
    let tx_speed = cmd.local_tx_speed;
    let udp = Arc::new(udp);

    let state_tx = state.clone();
    let udp_tx = udp.clone();
    let tx_handle = if client_should_tx {
        Some(tokio::spawn(async move {
            udp_client_tx_loop(&udp_tx, tx_size, tx_speed, state_tx).await
        }))
    } else {
        None
    };

    let state_rx = state.clone();
    let udp_rx = udp.clone();
    let rx_handle = if client_should_rx {
        Some(tokio::spawn(async move {
            udp_client_rx_loop(&udp_rx, state_rx).await
        }))
    } else {
        None
    };

    udp_client_status_loop(stream, cmd, &state).await;

    state.running.store(false, Ordering::SeqCst);
    if let Some(h) = tx_handle {
        let _ = h.await;
    }
    if let Some(h) = rx_handle {
        let _ = h.await;
    }
    Ok(())
}

fn bind_udp_socket(bind_addr: SocketAddr, spec: Option<&IfaceSpec>) -> Result<UdpSocket> {
    let ipv6 = bind_addr.is_ipv6();
    let domain = if ipv6 {
        socket2::Domain::IPV6
    } else {
        socket2::Domain::IPV4
    };
    let sock = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    sock.set_nonblocking(true)?;
    let _ = sock.set_send_buffer_size(4 * 1024 * 1024);
    let _ = sock.set_recv_buffer_size(4 * 1024 * 1024);
    if ipv6 {
        let _ = sock.set_only_v6(true);
    }
    if let Some(spec) = spec {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            crate::iface::pin_fd(sock.as_raw_fd(), spec, ipv6)?;
        }
        #[cfg(not(unix))]
        {
            let _ = spec;
        }
    }
    sock.bind(&bind_addr.into())?;
    if !bind_addr.ip().is_unspecified() {
        tracing::info!("UDP socket bound to {bind_addr}");
    }
    Ok(UdpSocket::from_std(sock.into())?)
}

async fn udp_client_tx_loop(
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
            Err(e) => {
                consecutive_errors += 1;
                if consecutive_errors == 1 {
                    tracing::debug!("UDP TX send error: {} (target)", e);
                }
                if consecutive_errors > 50000 {
                    tracing::warn!("UDP TX: too many consecutive send errors, stopping");
                    break;
                }
                let backoff = Duration::from_micros(
                    (200 + consecutive_errors.min(5000) as u64 * 10).min(10000),
                );
                tokio::time::sleep(backoff).await;
                continue;
            }
        }

        if state.tx_speed_changed.load(Ordering::Relaxed) {
            state.tx_speed_changed.store(false, Ordering::Relaxed);
            let new_speed = state.tx_speed.load(Ordering::Relaxed);
            interval = bandwidth::calc_send_interval(new_speed, tx_size as u16);
            next_send = Instant::now();
            tracing::debug!(
                "TX speed adjusted to {} bps ({:.2} Mbps)",
                new_speed,
                new_speed as f64 / 1_000_000.0
            );
        }

        match interval {
            Some(iv) => {
                let now = Instant::now();
                if let Some(delay) = bandwidth::advance_next_send(&mut next_send, iv, now) {
                    tokio::time::sleep(delay).await;
                }
            }
            None => {
                if seq % 64 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        }
    }
}

async fn udp_client_rx_loop(socket: &UdpSocket, state: Arc<BandwidthState>) {
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
                        state
                            .rx_lost_packets
                            .fetch_add(lost as u64, Ordering::Relaxed);
                    }
                }
                last_seq = Some(seq);
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

// --- Status Loops ---

async fn client_status_loop(cmd: &Command, state: &BandwidthState) {
    let mut seq: u32 = 0;
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;
        if !state.running.load(Ordering::Relaxed) {
            break;
        }

        seq += 1;

        let tx = if cmd.client_tx() {
            state.tx_bytes.swap(0, Ordering::Relaxed)
        } else {
            0
        };
        let rx = if cmd.client_rx() {
            state.rx_bytes.swap(0, Ordering::Relaxed)
        } else {
            0
        };
        state.record_interval(tx, rx, 0);

        let local_cpu = crate::cpu::get();
        let remote_cpu = state.remote_cpu.load(Ordering::Relaxed);
        if cmd.client_tx() {
            bandwidth::print_status_with_cpu(
                seq,
                "TX",
                tx,
                Duration::from_secs(1),
                None,
                Some(local_cpu),
                Some(remote_cpu),
            );
        }
        if cmd.client_rx() {
            bandwidth::print_status_with_cpu(
                seq,
                "RX",
                rx,
                Duration::from_secs(1),
                None,
                Some(local_cpu),
                Some(remote_cpu),
            );
        }
    }
}

/// UDP status exchange - sequential like C pselect():
///   1. Wait up to 1 second for server status
///   2. Read and process if available
///   3. ALWAYS send our status
async fn udp_client_status_loop(stream: &mut TcpStream, cmd: &Command, state: &BandwidthState) {
    let mut seq: u32 = 0;
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut status_buf = [0u8; STATUS_MSG_SIZE];
    let mut next_status = Instant::now() + Duration::from_secs(1);

    loop {
        if !state.running.load(Ordering::Relaxed) {
            break;
        }

        let now = Instant::now();
        let wait_time = if next_status > now {
            next_status - now
        } else {
            Duration::ZERO
        };

        match tokio::time::timeout(wait_time, reader.read_exact(&mut status_buf)).await {
            Ok(Ok(_)) => {
                let server_status = StatusMessage::deserialize(&status_buf);
                state
                    .remote_cpu
                    .store(server_status.cpu_load, Ordering::Relaxed);

                if server_status.bytes_received > 0 && cmd.client_tx() {
                    let new_speed = ((server_status.bytes_received as u64 * 8 * 3) / 2) as u32;
                    state.tx_speed.store(new_speed, Ordering::Relaxed);
                    state.tx_speed_changed.store(true, Ordering::Relaxed);
                    tracing::debug!(
                        "Server received {} bytes → TX {:.2} Mbps",
                        server_status.bytes_received,
                        new_speed as f64 / 1_000_000.0,
                    );
                }

                if Instant::now() < next_status {
                    continue;
                }
            }
            Ok(Err(_)) => {
                state.running.store(false, Ordering::SeqCst);
                break;
            }
            Err(_) => {}
        }

        // ALWAYS send our status every 1 second
        seq += 1;
        next_status = Instant::now() + Duration::from_secs(1);

        let rx_bytes = state.rx_bytes.swap(0, Ordering::Relaxed);
        let tx_bytes = state.tx_bytes.swap(0, Ordering::Relaxed);
        let lost = state.rx_lost_packets.swap(0, Ordering::Relaxed);
        state.record_interval(tx_bytes, rx_bytes, lost);

        let status = StatusMessage {
            cpu_load: crate::cpu::get(),
            seq,
            bytes_received: rx_bytes as u32,
        };
        if writer.write_all(&status.serialize()).await.is_err() {
            state.running.store(false, Ordering::SeqCst);
            break;
        }
        let _ = writer.flush().await;

        let local_cpu = crate::cpu::get();
        let remote_cpu = state.remote_cpu.load(Ordering::Relaxed);
        if cmd.client_tx() {
            bandwidth::print_status_with_cpu(
                seq,
                "TX",
                tx_bytes,
                Duration::from_secs(1),
                None,
                Some(local_cpu),
                Some(remote_cpu),
            );
        }
        if cmd.client_rx() {
            bandwidth::print_status_with_cpu(
                seq,
                "RX",
                rx_bytes,
                Duration::from_secs(1),
                Some(lost),
                Some(local_cpu),
                Some(remote_cpu),
            );
        }
    }
}
