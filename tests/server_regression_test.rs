//! Raw protocol regressions for session-wide TCP upload accounting.
//!
//! Only local simulators are used; no real router or credentials are required.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use btest_rs::protocol::{
    Command, StatusMessage, CMD_DIR_RX, CMD_PROTO_TCP,
    HELLO, STATUS_MSG_SIZE, STATUS_MSG_TYPE,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

const IO_TIMEOUT: Duration = Duration::from_millis(2500);
// The syslog destination is process-global; serialize these local collectors
// even when the test harness runs tests concurrently.
static ACCOUNTING_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct TestServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl TestServer {
    fn start(ipv6: bool) -> Self {
        let ip: IpAddr = if ipv6 {
            Ipv6Addr::LOCALHOST.into()
        } else {
            Ipv4Addr::LOCALHOST.into()
        };
        let reservation = std::net::TcpListener::bind(SocketAddr::new(ip, 0))
            .expect("reserve a local TCP port");
        let address = reservation.local_addr().unwrap();
        drop(reservation);
        let task = tokio::spawn(async move {
            let v4 = (!ipv6).then(|| "127.0.0.1".to_owned());
            let v6 = ipv6.then(|| "::1".to_owned());
            btest_rs::server::run_server(address.port(), None, None, false, v4, v6)
                .await
                .expect("local server must stay available");
        });
        Self { address, task }
    }

    async fn connect(&self) -> TcpStream {
        timeout(IO_TIMEOUT, async {
            loop {
                match TcpStream::connect(self.address).await {
                    Ok(stream) => return stream,
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                        sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("connect to local simulator: {error}"),
                }
            }
        })
        .await
        .expect("local server startup took more than 2.5 seconds")
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_exact<const N: usize>(stream: &mut TcpStream, phase: &str) -> [u8; N] {
    let mut buffer = [0u8; N];
    timeout(IO_TIMEOUT, stream.read_exact(&mut buffer))
        .await
        .unwrap_or_else(|_| panic!("{phase}: read exceeded 2.5 seconds"))
        .unwrap_or_else(|error| panic!("{phase}: {error}"));
    buffer
}

async fn write_all(stream: &mut TcpStream, bytes: &[u8], phase: &str) {
    timeout(IO_TIMEOUT, stream.write_all(bytes))
        .await
        .unwrap_or_else(|_| panic!("{phase}: write exceeded 2.5 seconds"))
        .unwrap_or_else(|error| panic!("{phase}: {error}"));
}

async fn open_primary(server: &TestServer, command: &Command) -> (TcpStream, [u8; 4]) {
    let mut stream = server.connect().await;
    stream.set_nodelay(true).unwrap();
    assert_eq!(read_exact::<4>(&mut stream, "hello").await, HELLO);
    write_all(&mut stream, &command.serialize(), "command").await;
    let response = read_exact::<4>(&mut stream, "authentication").await;
    assert_eq!(response[0], 1, "no-auth session must be accepted");
    (stream, response)
}

async fn status(stream: &mut TcpStream) -> StatusMessage {
    let wire = read_exact::<STATUS_MSG_SIZE>(stream, "status").await;
    assert_eq!(wire[0], STATUS_MSG_TYPE, "expected a protocol status frame");
    StatusMessage::deserialize(&wire)
}

async fn shutdown(mut stream: TcpStream) {
    timeout(IO_TIMEOUT, stream.shutdown())
        .await
        .expect("TCP shutdown exceeded 2.5 seconds")
        .expect("TCP shutdown failed");
}

async fn summary(syslog: &UdpSocket, peer: SocketAddr) -> String {
    timeout(IO_TIMEOUT, async {
        let mut buffer = [0u8; 4096];
        loop {
            let (length, _) = syslog.recv_from(&mut buffer).await.expect("local syslog receive");
            let message = String::from_utf8_lossy(&buffer[..length]).into_owned();
            if message.contains(&format!("TEST_END peer={peer} ")) {
                return message;
            }
        }
    })
    .await
    .expect("server summary must follow client disconnect within 2.5 seconds")
}

fn summary_value<'a>(message: &'a str, key: &str) -> &'a str {
    message.split_whitespace()
        .find_map(|field| field.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("summary lacks {key}: {message}"))
}

async fn tcp_upload_accounting(connection_count: usize, ipv6: bool) {
    let _serial = ACCOUNTING_TEST_LOCK.lock().await;
    let syslog = UdpSocket::bind("127.0.0.1:0").await.expect("local syslog collector");
    btest_rs::syslog_logger::init(&syslog.local_addr().unwrap().to_string())
        .expect("initialize local-only syslog");
    let server = TestServer::start(ipv6);
    let mut command = Command::new(CMD_PROTO_TCP, CMD_DIR_RX);
    if connection_count > 1 {
        command.tcp_conn_count = connection_count as u8;
    }
    let (primary, token) = open_primary(&server, &command).await;
    let peer = primary.local_addr().unwrap();
    let started = Instant::now();
    let mut streams = vec![primary];
    if connection_count > 1 {
        assert!(token[1] != 0 && token[2] != 0, "missing multi-stream session token");
        // AUTH_OK precedes registration of the primary's session. Allow that
        // registration to finish before submitting the secondary raw tokens.
        sleep(Duration::from_millis(30)).await;
        for _ in 1..connection_count {
            let mut stream = server.connect().await;
            stream.set_nodelay(true).unwrap();
            assert_eq!(read_exact::<4>(&mut stream, "secondary hello").await, HELLO);
            let mut join = [0u8; 16];
            join[0] = token[1];
            join[1] = token[2];
            join[2] = CMD_DIR_TX_JOIN;
            write_all(&mut stream, &join, "secondary join").await;
            assert_eq!(read_exact::<4>(&mut stream, "secondary authentication").await, token);
            streams.push(stream);
        }
    }

    // Synchronize with the first empty interval rather than assuming when the
    // server finishes collecting all secondary connections.
    let mut previous_sequence = Vec::new();
    for stream in &mut streams {
        let initial = status(stream).await;
        assert_eq!(initial.bytes_received, 0, "no upload data has been sent yet");
        previous_sequence.push(initial.seq);
    }

    let mut expected_total = vec![0u64; connection_count];
    let mut reported_total = vec![0u64; connection_count];
    for round in 0..2usize {
        let expected: Vec<usize> = (0..connection_count)
            .map(|index| (index + 1) * (997 + round * 211) + 13 + round * 31)
            .collect();
        for (index, stream) in streams.iter_mut().enumerate() {
            let payload = vec![0xA0 + (index % 32) as u8; expected[index]];
            write_all(stream, &payload, "per-stream upload").await;
            expected_total[index] += expected[index] as u64;
        }
        // RouterOS consumes server-level receive statistics rather than
        // summing independent per-connection reports. The primary alone must
        // describe all physical uploads; secondary reports carry the same
        // aggregate without consuming another reporter's interval counters.
        let expected_interval: usize = expected.iter().sum();
        for (index, stream) in streams.iter_mut().enumerate() {
            let report = status(stream).await;
            assert_eq!(report.seq, previous_sequence[index] + 1, "status interval skipped for stream {index}");
            assert_eq!(report.bytes_received as usize, expected_interval, "stream {index}/{connection_count}, round {round}: a RouterOS reader must see all uploaded bytes from any single status channel");
            reported_total[index] += report.bytes_received as u64;
            previous_sequence[index] = report.seq;
        }
    }
    let physical_total: u64 = expected_total.iter().sum();
    assert_eq!(reported_total[0], physical_total, "a RouterOS client consuming only primary status must account for every uploaded byte");
    assert!(reported_total.iter().all(|total| *total == physical_total), "each status channel must report the same physical aggregate; duplicate status messages are not additional data bytes");
    for stream in streams {
        shutdown(stream).await;
    }
    let result = summary(&syslog, peer).await;
    let duration: u32 = summary_value(&result, "duration").trim_end_matches('s').parse().unwrap();
    let rx_bytes: u64 = summary_value(&result, "rx_bytes").parse().unwrap();
    let tx_bytes: u64 = summary_value(&result, "tx_bytes").parse().unwrap();
    assert_eq!(rx_bytes, expected_total.iter().sum::<u64>(), "aggregate server summary must conserve all stream bytes");
    assert_eq!(tx_bytes, 0, "server is receiving only");
    assert!((2..=5).contains(&duration), "duration must follow approximately 3-4 seconds of real time, not multiply by {connection_count} streams: {result}");
    assert!(f64::from(duration) <= started.elapsed().as_secs_f64() + 1.0, "summary overstates actual wall-clock duration: {result}");
}

// Raw secondary join commands use the historical protocol's marker byte 0x02;
// unlike a primary command, bytes 0-1 contain the session token, not proto/dir.
const CMD_DIR_TX_JOIN: u8 = 0x02;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_single_upload_ipv4_reports_all_bytes() {
    tcp_upload_accounting(1, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_two_stream_upload_primary_reports_session_aggregate() {
    tcp_upload_accounting(2, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_twenty_stream_upload_primary_reports_session_aggregate() {
    tcp_upload_accounting(20, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_single_upload_ipv6_reports_all_bytes() {
    tcp_upload_accounting(1, true).await;
}
