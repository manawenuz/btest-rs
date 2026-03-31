use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const SERVER_PORT: u16 = 12000;

async fn start_test_server(port: u16, auth_user: Option<&str>, auth_pass: Option<&str>) {
    let user = auth_user.map(String::from);
    let pass = auth_pass.map(String::from);
    tokio::spawn(async move {
        let _ = btest_rs::server::run_server(port, user, pass, false).await;
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
        )
        .await
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    handle.abort();
}
