# btest-rs Architecture

## Overview

btest-rs is a Rust reimplementation of the MikroTik Bandwidth Test protocol. It operates in two modes: **server** (accepts connections from MikroTik devices) and **client** (connects to MikroTik btest servers).

## Module Structure

```mermaid
graph TB
    main["main.rs<br/>CLI parsing (clap)"]
    server["server.rs<br/>Server mode"]
    client["client.rs<br/>Client mode"]
    protocol["protocol.rs<br/>Wire protocol types"]
    auth["auth.rs<br/>MD5 authentication"]
    ecsrp5["ecsrp5.rs<br/>EC-SRP5 authentication<br/>(Curve25519 Weierstrass)"]
    bandwidth["bandwidth.rs<br/>Rate control & reporting"]
    csv_output["csv_output.rs<br/>CSV result logging"]
    syslog["syslog_logger.rs<br/>Remote syslog (RFC 3164)"]
    lib["lib.rs<br/>Public API for tests"]

    main --> server
    main --> client
    main --> bandwidth
    main --> csv_output
    main --> syslog
    server --> protocol
    server --> auth
    server --> ecsrp5
    server --> bandwidth
    server --> syslog
    client --> protocol
    client --> auth
    client --> ecsrp5
    client --> bandwidth
    lib --> server
    lib --> client
    lib --> protocol
    lib --> auth
    lib --> ecsrp5
    lib --> bandwidth
```

## Data Flow

### Server Mode (MikroTik connects to us)

```mermaid
sequenceDiagram
    participant MK as MikroTik Client
    participant TCP as TCP Control<br/>(port 2000)
    participant SRV as btest-rs Server
    participant UDP as UDP Data<br/>(port 2001+)

    MK->>TCP: Connect
    SRV->>TCP: HELLO [01 00 00 00]
    MK->>TCP: Command [16 bytes]
    Note over SRV: Parse proto, direction,<br/>tx_size, speeds

    alt No auth configured
        SRV->>TCP: AUTH_OK [01 00 00 00]
    else MD5 auth (RouterOS < 6.43)
        SRV->>TCP: AUTH_REQUIRED [02 00 00 00]
        SRV->>TCP: Challenge [16 random bytes]
        MK->>TCP: Response [16 hash + 32 username]
        Note over SRV: Verify MD5(pass + MD5(pass + challenge))
        SRV->>TCP: AUTH_OK or AUTH_FAILED
    else EC-SRP5 auth (RouterOS >= 6.43, --ecsrp5 flag)
        SRV->>TCP: EC-SRP5 [03 00 00 00]
        MK->>TCP: [len][username\0][client_pubkey:32][parity:1]
        SRV->>TCP: [len][server_pubkey:32][parity:1][salt:16]
        MK->>TCP: [len][client_confirmation:32]
        SRV->>TCP: [len][server_confirmation:32]
        Note over SRV: Curve25519 Weierstrass EC-SRP5<br/>See docs/ecsrp5-research.md
        SRV->>TCP: AUTH_OK [01 00 00 00]
    end

    alt TCP mode
        Note over SRV,MK: Data flows on same TCP connection
        loop Every second
            SRV-->>SRV: Print bandwidth stats
        end
    else UDP mode
        SRV->>TCP: UDP port [2 bytes BE]
        Note over SRV: Bind UDP socket
        par TX Thread (if server transmits)
            loop Continuous
                SRV->>UDP: Data packets [seq + payload]
            end
        and RX Thread (if server receives)
            loop Continuous
                UDP->>SRV: Data packets [seq + payload]
            end
        and Status Loop (TCP control)
            loop Every 1 second
                MK->>TCP: Status [12 bytes]
                SRV->>TCP: Status [12 bytes]
                Note over SRV: Adjust TX speed<br/>based on client feedback
            end
        end
    end
```

### Client Mode (we connect to MikroTik)

```mermaid
sequenceDiagram
    participant CLI as btest-rs Client
    participant TCP as TCP Control
    participant MK as MikroTik Server

    CLI->>TCP: Connect to MikroTik:2000
    MK->>TCP: HELLO
    CLI->>TCP: Command [16 bytes]
    Note over CLI: direction bits tell server<br/>what to do (TX/RX/BOTH)

    alt Auth response 01 (no auth)
        Note over CLI: No auth, proceed
    else Auth response 02 (MD5)
        MK->>TCP: Challenge [16 random bytes]
        CLI->>TCP: MD5 response [48 bytes]
        MK->>TCP: AUTH_OK
    else Auth response 03 (EC-SRP5)
        CLI->>TCP: [len][username\0][client_pubkey:32][parity:1]
        MK->>TCP: [len][server_pubkey:32][parity:1][salt:16]
        CLI->>TCP: [len][client_confirmation:32]
        MK->>TCP: [len][server_confirmation:32]
        MK->>TCP: AUTH_OK
    end

    Note over CLI,MK: Data transfer begins<br/>(TCP or UDP, same as server)
```

## Threading Model

```mermaid
graph TB
    subgraph "Server Process"
        LISTEN["Main Loop<br/>Accept connections"]
        LISTEN -->|spawn per client| HANDLER

        subgraph "Per-Client Tasks (tokio)"
            HANDLER["Connection Handler<br/>Handshake + Auth"]
            HANDLER --> TX["TX Task<br/>Send data packets"]
            HANDLER --> RX["RX Task<br/>Receive data packets"]
            HANDLER --> STATUS["Status Loop<br/>Exchange stats every 1s"]
        end
    end

    subgraph "Shared State (Arc + Atomics)"
        STATE["BandwidthState"]
        TX_BYTES["tx_bytes: AtomicU64"]
        RX_BYTES["rx_bytes: AtomicU64"]
        TX_SPEED["tx_speed: AtomicU32"]
        RUNNING["running: AtomicBool"]
    end

    TX --> TX_BYTES
    RX --> RX_BYTES
    STATUS --> TX_BYTES
    STATUS --> RX_BYTES
    STATUS --> TX_SPEED
    TX --> TX_SPEED
    TX --> RUNNING
    RX --> RUNNING
    STATUS --> RUNNING
```

## Key Design Decisions

### 1. Tokio async runtime

All I/O is async via tokio. Each client connection spawns independent tasks for TX, RX, and status exchange. This allows handling hundreds of concurrent connections on a single thread pool.

### 2. Lock-free shared state

TX/RX threads and the status loop share bandwidth counters via `AtomicU64`. No mutexes needed -- `swap(0)` atomically reads and resets counters each interval.

### 3. Sequential status loop (matching C pselect)

The UDP status exchange uses a sequential timeout-read-then-send pattern rather than `tokio::select!`. This ensures our status messages are sent exactly every 1 second, preventing MikroTik's speed adaptation from seeing irregular feedback.

### 4. Direction bits from server perspective

The direction byte in the protocol means what the **server** should do:
- `0x01` (CMD_DIR_RX) = server receives
- `0x02` (CMD_DIR_TX) = server transmits
- `0x03` (CMD_DIR_BOTH) = bidirectional

The client inverts before sending: client "transmit" sends `CMD_DIR_RX` (telling server to receive).

### 5. TCP socket half keepalive

When only one direction is active (e.g., TX only), the unused socket half is kept alive. Dropping `OwnedWriteHalf` sends a TCP FIN, which MikroTik interprets as disconnection.

### 6. Static musl binary

Release builds use musl for a fully static binary with zero runtime dependencies. The binary is approximately 2 MB and runs on any Linux distribution.

### 7. EC-SRP5 with big integer arithmetic

The EC-SRP5 implementation uses `num-bigint` for Curve25519 Weierstrass-form elliptic curve arithmetic. MikroTik's authentication uses the Weierstrass form (not the more common Montgomery or Edwards forms), requiring direct field arithmetic over the prime `2^255 - 19`. The implementation includes point multiplication, `lift_x`, `redp1` (hash-to-curve), and Montgomery coordinate conversion.

### 8. Global singletons for syslog and CSV

The syslog and CSV modules use `Mutex<Option<...>>` global statics. This avoids threading state through every function call while remaining safe. Both modules are initialized once at startup and used from any async task via their public API functions.

## File Layout

```
btest-rs/
├── src/
│   ├── main.rs              # CLI entry point, argument parsing (clap)
│   ├── lib.rs               # Public API (used by integration tests)
│   ├── protocol.rs          # Wire format: Command, StatusMessage, constants
│   ├── auth.rs              # MD5 challenge-response authentication
│   ├── ecsrp5.rs            # EC-SRP5 authentication (Curve25519 Weierstrass)
│   ├── server.rs            # Server mode: listener, TCP/UDP handlers
│   ├── client.rs            # Client mode: connector, TCP/UDP handlers
│   ├── bandwidth.rs         # Rate limiting, formatting, shared state
│   ├── csv_output.rs        # CSV result logging (append-mode, auto-header)
│   └── syslog_logger.rs     # Remote syslog sender (RFC 3164 / BSD format)
├── tests/
│   └── integration_test.rs  # End-to-end server/client tests
├── scripts/
│   ├── build-linux.sh           # Cross-compile for x86_64 Linux (musl)
│   ├── build-macos-release.sh   # macOS release build
│   ├── install-service.sh       # systemd service installer
│   ├── push-docker.sh           # Push Docker image to registry
│   ├── test-local.sh            # Loopback self-test
│   ├── test-mikrotik.sh         # Test against MikroTik device
│   ├── test-docker.sh           # Docker container test
│   └── debug-capture.sh         # Packet capture for debugging
├── docs/
│   ├── architecture.md          # This file
│   ├── protocol.md              # Protocol specification
│   ├── user-guide.md            # Usage documentation
│   ├── docker.md                # Docker & deployment guide
│   ├── ecsrp5-research.md       # EC-SRP5 reverse-engineering notes
│   └── man/
│       └── btest.1              # Unix manual page (troff format)
├── Dockerfile                   # Production Docker image (multi-stage)
├── Dockerfile.cross             # Cross-compilation for Linux x86_64
├── docker-compose.yml           # Docker Compose configuration
├── Cargo.toml                   # Rust package manifest
├── Cargo.lock                   # Dependency lock file
├── LICENSE                      # MIT License
└── btest-opensource/            # Original C implementation (git submodule)
```
