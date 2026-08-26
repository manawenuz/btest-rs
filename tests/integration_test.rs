use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const SERVER_PORT: u16 = 12000;

async fn start_test_server(port: u16, auth_user: Option<&str>, auth_pass: Option<&str>) {
    let user = auth_user.map(String::from);
    let pass = auth_pass.map(String::from);
    tokio::spawn(async move {
        let _ = btest_rs::server::run_server(port, user, pass, false, Some("127.0.0.1".into()), None).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn test_server_hello() {
    let port = SERVER_PORT;
    start_test_server(port, None, None).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Failed to connect");

    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x01, 0x00, 0x00, 0x00], "Expected HELLO response");
}

#[tokio::test]
async fn test_server_command_and_noauth() {
    let port = SERVER_PORT + 1;
    start_test_server(port, None, None).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Failed to connect");

    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x01, 0x00, 0x00, 0x00]);

    // CMD_DIR_TX (0x02) = server should transmit data to us
    let cmd = btest_rs::protocol::Command::new(
        btest_rs::protocol::CMD_PROTO_TCP,
        btest_rs::protocol::CMD_DIR_TX,
    );
    stream.write_all(&cmd.serialize()).await.unwrap();
    stream.flush().await.unwrap();

    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x01, 0x00, 0x00, 0x00], "Expected AUTH_OK");

    // Server should start sending data
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut data = vec![0u8; 4096];
    let n = stream.read(&mut data).await.unwrap();
    assert!(n > 0, "Expected to receive data from server");
}

#[tokio::test]
async fn test_server_auth_challenge() {
    let port = SERVER_PORT + 2;
    start_test_server(port, Some("admin"), Some("test")).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Failed to connect");

    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x01, 0x00, 0x00, 0x00]);

    // CMD_DIR_TX = server transmits
    let cmd = btest_rs::protocol::Command::new(
        btest_rs::protocol::CMD_PROTO_TCP,
        btest_rs::protocol::CMD_DIR_TX,
    );
    stream.write_all(&cmd.serialize()).await.unwrap();
    stream.flush().await.unwrap();

    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x02, 0x00, 0x00, 0x00], "Expected AUTH_REQUIRED");

    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge).await.unwrap();

    let hash = btest_rs::auth::compute_auth_hash("test", &challenge);
    let mut response = [0u8; 48];
    response[0..16].copy_from_slice(&hash);
    response[16..21].copy_from_slice(b"admin");

    stream.write_all(&response).await.unwrap();
    stream.flush().await.unwrap();

    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x01, 0x00, 0x00, 0x00], "Expected AUTH_OK");
}

#[tokio::test]
async fn test_server_auth_failure() {
    let port = SERVER_PORT + 3;
    start_test_server(port, Some("admin"), Some("test")).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Failed to connect");

    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();

    let cmd = btest_rs::protocol::Command::new(
        btest_rs::protocol::CMD_PROTO_TCP,
        btest_rs::protocol::CMD_DIR_TX,
    );
    stream.write_all(&cmd.serialize()).await.unwrap();
    stream.flush().await.unwrap();

    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x02, 0x00, 0x00, 0x00]);

    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge).await.unwrap();

    let hash = btest_rs::auth::compute_auth_hash("wrongpassword", &challenge);
    let mut response = [0u8; 48];
    response[0..16].copy_from_slice(&hash);
    response[16..21].copy_from_slice(b"admin");

    stream.write_all(&response).await.unwrap();
    stream.flush().await.unwrap();

    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x00, 0x00, 0x00, 0x00], "Expected AUTH_FAILED");
}

// Loopback tests use run_client which builds direction correctly
// (client transmit → CMD_DIR_RX, client receive → CMD_DIR_TX)

#[tokio::test]
async fn test_loopback_tcp_rx() {
    let port = SERVER_PORT + 4;
    start_test_server(port, None, None).await;

    let handle = tokio::spawn(async move {
        btest_rs::client::run_client(
            "127.0.0.1",
            port,
            btest_rs::protocol::CMD_DIR_TX, // server TX = client RX
            false,
            0,
            0,
            None,
            None,
            false,
            btest_rs::bandwidth::BandwidthState::new(),
        )
        .await
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    handle.abort();
}

#[tokio::test]
async fn test_loopback_tcp_tx() {
    let port = SERVER_PORT + 5;
    start_test_server(port, None, None).await;

    let handle = tokio::spawn(async move {
        btest_rs::client::run_client(
            "127.0.0.1",
            port,
            btest_rs::protocol::CMD_DIR_RX, // server RX = client TX
            false,
            0,
            0,
            None,
            None,
            false,
            btest_rs::bandwidth::BandwidthState::new(),
        )
        .await
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    handle.abort();
}

#[tokio::test]
async fn test_loopback_tcp_both() {
    let port = SERVER_PORT + 6;
    start_test_server(port, None, None).await;

    let handle = tokio::spawn(async move {
        btest_rs::client::run_client(
            "127.0.0.1",
            port,
            btest_rs::protocol::CMD_DIR_BOTH,
            false,
            0,
            0,
            None,
            None,
            false,
            btest_rs::bandwidth::BandwidthState::new(),
        )
        .await
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    handle.abort();
}

#[tokio::test]
async fn test_loopback_tcp_with_auth() {
    let port = SERVER_PORT + 7;
    start_test_server(port, Some("admin"), Some("secret")).await;

    let handle = tokio::spawn(async move {
        btest_rs::client::run_client(
            "127.0.0.1",
            port,
            btest_rs::protocol::CMD_DIR_TX, // server TX = client RX
            false,
            0,
            0,
            Some("admin".into()),
            Some("secret".into()),
            false,
            btest_rs::bandwidth::BandwidthState::new(),
        )
        .await
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    handle.abort();
}

async fn open_multi_primary(port: u16) -> (TcpStream, [u8; 2]) {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Failed to connect primary stream");

    let mut response = [0u8; 4];
    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(response, [0x01, 0x00, 0x00, 0x00]);

    let mut cmd = btest_rs::protocol::Command::new(
        btest_rs::protocol::CMD_PROTO_TCP,
        btest_rs::protocol::CMD_DIR_TX,
    );
    cmd.tcp_conn_count = 2;
    stream.write_all(&cmd.serialize()).await.unwrap();
    stream.flush().await.unwrap();

    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(response[0], 0x01);
    assert_eq!(response[3], 0x00);
    assert_ne!([response[1], response[2]], [0, 0]);
    (stream, [response[1], response[2]])
}

async fn join_multi_secondary(port: u16, token: [u8; 2]) -> TcpStream {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Failed to connect secondary stream");

    let mut response = [0u8; 4];
    stream.read_exact(&mut response).await.unwrap();

    let mut join = [0u8; 16];
    join[0..2].copy_from_slice(&token);
    join[2] = 0x02;
    stream.write_all(&join).await.unwrap();
    stream.flush().await.unwrap();

    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(response, [0x01, token[0], token[1], 0x00]);
    stream
}

#[tokio::test]
async fn test_two_multiconnection_clients_from_same_ip_are_independent() {
    let port = SERVER_PORT + 8;
    start_test_server(port, None, None).await;

    // Both primaries originate from 127.0.0.1. They must receive distinct
    // sessions instead of the second primary being mistaken for a secondary
    // connection belonging to the first test.
    let (mut primary_a, token_a) = open_multi_primary(port).await;
    let (mut primary_b, token_b) = open_multi_primary(port).await;
    assert_ne!(token_a, token_b);

    let mut secondary_a = join_multi_secondary(port, token_a).await;
    let mut secondary_b = join_multi_secondary(port, token_b).await;

    let mut byte = [0u8; 1];
    for stream in [&mut primary_a, &mut primary_b, &mut secondary_a, &mut secondary_b] {
        tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut byte))
            .await
            .expect("multi-connection stream did not start")
            .unwrap();
    }
}
