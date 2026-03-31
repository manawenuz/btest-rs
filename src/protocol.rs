use std::io;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// --- Constants ---

pub const BTEST_PORT: u16 = 2000;
pub const BTEST_UDP_PORT_START: u16 = 2001;
pub const BTEST_PORT_CLIENT_OFFSET: u16 = 256;

pub const CMD_PROTO_UDP: u8 = 0x00;
pub const CMD_PROTO_TCP: u8 = 0x01;

pub const CMD_DIR_RX: u8 = 0x01;
pub const CMD_DIR_TX: u8 = 0x02;
pub const CMD_DIR_BOTH: u8 = 0x03;

pub const DEFAULT_TCP_TX_SIZE: u16 = 0x8000; // 32768
pub const DEFAULT_UDP_TX_SIZE: u16 = 0x05DC; // 1500

pub const HELLO: [u8; 4] = [0x01, 0x00, 0x00, 0x00];
pub const AUTH_OK: [u8; 4] = [0x01, 0x00, 0x00, 0x00];
pub const AUTH_REQUIRED: [u8; 4] = [0x02, 0x00, 0x00, 0x00];
pub const AUTH_FAILED: [u8; 4] = [0x00, 0x00, 0x00, 0x00];

pub const STATUS_MSG_TYPE: u8 = 0x07;
pub const STATUS_MSG_SIZE: usize = 12;

// --- Error Types ---

#[derive(Error, Debug)]
pub enum BtestError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Authentication failed")]
    AuthFailed,
    #[error("Invalid command")]
    InvalidCommand,
}

pub type Result<T> = std::result::Result<T, BtestError>;

// --- Command Structure ---

#[derive(Debug, Clone)]
pub struct Command {
    pub proto: u8,
    pub direction: u8,
    pub random_data: u8,
    pub tcp_conn_count: u8,
    pub tx_size: u16,
    pub client_buf_size: u16,
    pub remote_tx_speed: u32,
    pub local_tx_speed: u32,
}

impl Command {
    pub fn new(proto: u8, direction: u8) -> Self {
        let tx_size = if proto == CMD_PROTO_UDP {
            DEFAULT_UDP_TX_SIZE
        } else {
            DEFAULT_TCP_TX_SIZE
        };
        Self {
            proto,
            direction,
            random_data: 0,
            tcp_conn_count: 0,
            tx_size,
            client_buf_size: 0,
            remote_tx_speed: 0,
            local_tx_speed: 0,
        }
    }

    pub fn serialize(&self) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0] = self.proto;
        buf[1] = self.direction;
        buf[2] = self.random_data;
        buf[3] = self.tcp_conn_count;
        buf[4..6].copy_from_slice(&self.tx_size.to_le_bytes());
        buf[6..8].copy_from_slice(&self.client_buf_size.to_le_bytes());
        buf[8..12].copy_from_slice(&self.remote_tx_speed.to_le_bytes());
        buf[12..16].copy_from_slice(&self.local_tx_speed.to_le_bytes());
        buf
    }

    pub fn deserialize(buf: &[u8; 16]) -> Self {
        Self {
            proto: buf[0],
            direction: buf[1],
            random_data: buf[2],
            tcp_conn_count: buf[3],
            tx_size: u16::from_le_bytes([buf[4], buf[5]]),
            client_buf_size: u16::from_le_bytes([buf[6], buf[7]]),
            remote_tx_speed: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            local_tx_speed: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
        }
    }

    pub fn is_udp(&self) -> bool {
        self.proto == CMD_PROTO_UDP
    }

    // Direction bits are from SERVER's perspective:
    //   CMD_DIR_RX (0x01) = server receives
    //   CMD_DIR_TX (0x02) = server transmits
    // Client inverts when building: client TX → CMD_DIR_RX, client RX → CMD_DIR_TX

    /// Server should transmit (CMD_DIR_TX bit set)
    pub fn server_tx(&self) -> bool {
        self.direction & CMD_DIR_TX != 0
    }

    /// Server should receive (CMD_DIR_RX bit set)
    pub fn server_rx(&self) -> bool {
        self.direction & CMD_DIR_RX != 0
    }

    /// Client should transmit (inverse: CMD_DIR_RX bit = server receives our data)
    pub fn client_tx(&self) -> bool {
        self.direction & CMD_DIR_RX != 0
    }

    /// Client should receive (inverse: CMD_DIR_TX bit = server sends us data)
    pub fn client_rx(&self) -> bool {
        self.direction & CMD_DIR_TX != 0
    }
}

// --- Status Message ---

#[derive(Debug, Clone, Default)]
pub struct StatusMessage {
    pub seq: u32,
    pub bytes_received: u32,
}

impl StatusMessage {
    pub fn serialize(&self) -> [u8; STATUS_MSG_SIZE] {
        let mut buf = [0u8; STATUS_MSG_SIZE];
        buf[0] = STATUS_MSG_TYPE;
        buf[1..5].copy_from_slice(&self.seq.to_be_bytes());
        buf[5] = 0;
        buf[6] = 0;
        buf[7] = 0;
        buf[8..12].copy_from_slice(&self.bytes_received.to_le_bytes());
        buf
    }

    pub fn deserialize(buf: &[u8; STATUS_MSG_SIZE]) -> Self {
        Self {
            seq: u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]),
            bytes_received: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        }
    }
}

// --- Protocol Helpers ---

pub async fn send_hello<W: AsyncWriteExt + Unpin>(writer: &mut W) -> Result<()> {
    writer.write_all(&HELLO).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn recv_hello<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<()> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).await?;
    if buf != HELLO {
        return Err(BtestError::Protocol(format!(
            "Expected HELLO {:02x?}, got {:02x?}",
            HELLO, buf
        )));
    }
    Ok(())
}

pub async fn send_command<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    cmd: &Command,
) -> Result<()> {
    writer.write_all(&cmd.serialize()).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn recv_command<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Command> {
    let mut buf = [0u8; 16];
    reader.read_exact(&mut buf).await?;
    let cmd = Command::deserialize(&buf);
    if cmd.proto > 1 || cmd.direction == 0 || cmd.direction > 3 {
        return Err(BtestError::InvalidCommand);
    }
    Ok(cmd)
}

pub async fn recv_response<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<[u8; 4]> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}
