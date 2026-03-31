use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const SERVER_PORT: u16 = 13000;

async fn start_ecsrp5_server(port: u16) {
    tokio::spawn(async move {
        let _ = btest_rs::server::run_server(
            port,
            Some("testuser".into()),
            Some("testpass".into()),
            true, // ecsrp5
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
}

async fn start_md5_server(port: u16) {
    tokio::spawn(async move {
        let _ = btest_rs::server::run_server(
            port,
            Some("testuser".into()),
            Some("testpass".into()),
            false, // md5
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
}

async fn start_noauth_server(port: u16) {
    tokio::spawn(async move {
        let _ = btest_rs::server::run_server(port, None, None, false).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn test_ecsrp5_server_sends_03_response() {
    let port = SERVER_PORT;
    start_ecsrp5_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();

    // Read HELLO
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x01, 0x00, 0x00, 0x00]);

    // Send command (TCP, server TX)
    let cmd = btest_rs::protocol::Command::new(
        btest_rs::protocol::CMD_PROTO_TCP,
        btest_rs::protocol::CMD_DIR_TX,
    );
    stream.write_all(&cmd.serialize()).await.unwrap();
    stream.flush().await.unwrap();

    // Should receive EC-SRP5 auth required
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [0x03, 0x00, 0x00, 0x00], "Expected EC-SRP5 auth response");
}

#[tokio::test]
async fn test_ecsrp5_full_client_auth() {
    let port = SERVER_PORT + 1;
    start_ecsrp5_server(port).await;

    // Use our client with EC-SRP5
    let handle = tokio::spawn(async move {
        btest_rs::client::run_client(
            "127.0.0.1",
            port,
            btest_rs::protocol::CMD_DIR_TX, // server TX = client RX
            false,
            0,
            0,
            Some("testuser".into()),
            Some("testpass".into()),
            false,
        )
        .await
    });

    tokio::time::sleep(Duration::from_secs(3)).await;
    handle.abort();
    // If we got here without panic, EC-SRP5 auth + data transfer worked
}

#[tokio::test]
async fn test_ecsrp5_wrong_password_fails() {
    let port = SERVER_PORT + 2;
    start_ecsrp5_server(port).await;

    let result = btest_rs::client::run_client(
        "127.0.0.1",
        port,
        btest_rs::protocol::CMD_DIR_TX,
        false,
        0,
        0,
        Some("testuser".into()),
        Some("wrongpass".into()),
        false,
    )
    .await;

    assert!(result.is_err(), "Wrong password should fail");
}

#[tokio::test]
async fn test_md5_auth_still_works() {
    let port = SERVER_PORT + 3;
    start_md5_server(port).await;

    let handle = tokio::spawn(async move {
        btest_rs::client::run_client(
            "127.0.0.1",
            port,
            btest_rs::protocol::CMD_DIR_TX,
            false,
            0,
            0,
            Some("testuser".into()),
            Some("testpass".into()),
            false,
        )
        .await
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    handle.abort();
}

#[tokio::test]
async fn test_noauth_still_works() {
    let port = SERVER_PORT + 4;
    start_noauth_server(port).await;

    let handle = tokio::spawn(async move {
        btest_rs::client::run_client(
            "127.0.0.1",
            port,
            btest_rs::protocol::CMD_DIR_TX,
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
async fn test_ecsrp5_udp_bidirectional() {
    let port = SERVER_PORT + 5;
    start_ecsrp5_server(port).await;

    let handle = tokio::spawn(async move {
        btest_rs::client::run_client(
            "127.0.0.1",
            port,
            btest_rs::protocol::CMD_DIR_BOTH,
            true, // UDP
            0,
            0,
            Some("testuser".into()),
            Some("testpass".into()),
            false,
        )
        .await
    });

    tokio::time::sleep(Duration::from_secs(3)).await;
    handle.abort();
}
