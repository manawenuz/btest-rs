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
    bandwidth["bandwidth.rs<br/>Rate control & reporting"]
    lib["lib.rs<br/>Public API for tests"]

    main --> server
    main --> client
    main --> bandwidth
    server --> protocol
    server --> auth
    server --> bandwidth
    client --> protocol
    client --> auth
    client --> bandwidth
    lib --> server
    lib --> client
    lib --> protocol
    lib --> auth
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
    else MD5 auth
        SRV->>TCP: AUTH_REQUIRED [02 00 00 00]
        SRV->>TCP: Challenge [16 random bytes]
        MK->>TCP: Response [16 hash + 32 username]
        Note over SRV: Verify MD5(pass + MD5(pass + challenge))
        SRV->>TCP: AUTH_OK or AUTH_FAILED
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

    alt Auth response 01
        Note over CLI: No auth, proceed
    else Auth response 02 (MD5)
        MK->>TCP: Challenge
        CLI->>TCP: MD5 response
        MK->>TCP: AUTH_OK
    else Auth response 03 (EC-SRP5)
        Note over CLI: Not supported yet
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
TX/RX threads and the status loop share bandwidth counters via `AtomicU64`. No mutexes needed — `swap(0)` atomically reads and resets counters each interval.

### 3. Sequential status loop (matching C pselect)
The UDP status exchange uses a sequential timeout-read-then-send pattern rather than `tokio::select!`. This ensures our status messages are sent exactly every 1 second, preventing MikroTik's speed adaptation from seeing irregular feedback.

### 4. Direction bits from server perspective
The direction byte in the protocol means what the **server** should do:
- `0x01` (CMD_DIR_RX) = server receives
- `0x02` (CMD_DIR_TX) = server transmits
- `0x03` (CMD_DIR_BOTH) = bidirectional

The client inverts before sending: client "transmit" → `CMD_DIR_RX` (telling server to receive).

### 5. TCP socket half keepalive
When only one direction is active (e.g., TX only), the unused socket half is kept alive. Dropping `OwnedWriteHalf` sends a TCP FIN, which MikroTik interprets as disconnection.

### 6. Static musl binary
Release builds use musl for a fully static binary with zero runtime dependencies. The binary is 2 MB and runs on any Linux.

## File Layout

```
btest-rs/
├── src/
│   ├── main.rs          # CLI entry point, argument parsing
│   ├── lib.rs           # Public API (used by integration tests)
│   ├── protocol.rs      # Wire format: Command, StatusMessage, constants
│   ├── auth.rs          # MD5 challenge-response authentication
│   ├── server.rs        # Server mode: listener, TCP/UDP handlers
│   ├── client.rs        # Client mode: connector, TCP/UDP handlers
│   └── bandwidth.rs     # Rate limiting, formatting, shared state
├── tests/
│   └── integration_test.rs  # End-to-end server/client tests
├── scripts/
│   ├── build-linux.sh       # Cross-compile for x86_64 Linux
│   ├── install-service.sh   # systemd service installer
│   ├── test-local.sh        # Loopback self-test
│   ├── test-mikrotik.sh     # Test against MikroTik device
│   └── test-docker.sh       # Docker container test
├── docs/
│   ├── architecture.md      # This file
│   ├── protocol.md          # Protocol specification
│   ├── user-guide.md        # Usage documentation
│   └── docker.md            # Docker & deployment guide
├── Dockerfile               # Production Docker image
├── Dockerfile.cross         # Cross-compilation for Linux x86_64
├── docker-compose.yml       # Docker Compose configuration
├── Cargo.toml
└── btest-opensource/        # Original C implementation (git submodule)
```
