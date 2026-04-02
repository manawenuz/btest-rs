# btest-rs Architecture

## Overview

btest-rs is a Rust reimplementation of the MikroTik Bandwidth Test protocol. It operates in two modes: **server** (accepts connections from MikroTik devices) and **client** (connects to MikroTik btest servers). An optional **server-pro** mode adds multi-user support, quotas, and a web dashboard.

## Module Structure

```
src/
├── main.rs              # CLI entry point, argument parsing (clap)
├── lib.rs               # Public API (re-exports all modules for tests/pro)
├── protocol.rs          # Wire format: Command, StatusMessage, constants
├── auth.rs              # MD5 challenge-response authentication
├── ecsrp5.rs            # EC-SRP5 authentication (Curve25519 Weierstrass)
├── server.rs            # Server mode: listener, TCP/UDP handlers, multi-conn
├── client.rs            # Client mode: connector, TCP/UDP handlers, status parsing
├── bandwidth.rs         # Rate limiting, formatting, shared BandwidthState, byte budget
├── cpu.rs               # CPU sampler (macOS, Linux, Android, Windows, FreeBSD)
├── csv_output.rs        # CSV result logging (append-mode, auto-header)
├── syslog_logger.rs     # Remote syslog sender (RFC 3164 / BSD format)
├── bin/
│   ├── client_only.rs   # Stripped client binary for embedded/OpenWrt
│   └── server_only.rs   # Stripped server binary for embedded/OpenWrt
└── server_pro/          # Optional (--features pro)
    ├── main.rs          # Pro CLI: user management, quota flags, web port
    ├── server_loop.rs   # Accept loop with auth, quotas, multi-conn sessions
    ├── user_db.rs       # SQLite: users, usage, ip_usage, sessions, intervals
    ├── quota.rs         # QuotaManager: per-user + per-IP limits, remaining_budget()
    ├── enforcer.rs      # QuotaEnforcer: periodic checks, max_duration, StopReason
    ├── ldap_auth.rs     # LDAP auth scaffold (not yet wired)
    └── web/
        └── mod.rs       # Axum web dashboard: Chart.js, quota bars, JSON export
```

## CLI Output Format

The client outputs one line per second per direction:

```
[   5]  TX  285.47 Mbps (35684352 bytes)  cpu: 20%/62%
[   5]  RX  283.64 Mbps (35454988 bytes)  cpu: 20%/62%  lost: 12
```

Format: `[interval] direction speed (bytes) cpu: local%/remote% [lost: N]`

At test end, a summary line:
```
TEST_END peer=172.16.81.1 proto=TCP dir=both duration=60s tx_avg=284.94Mbps rx_avg=272.83Mbps tx_bytes=2137030656 rx_bytes=2046260728 lost=0
```

## Data Flow

### Server Mode (MikroTik connects to us)

```
MikroTik → TCP:2000 → HELLO → Command [16 bytes] → Auth → Data Transfer
```

1. Server sends HELLO `[01 00 00 00]`
2. Client sends 16-byte command (protocol, direction, tx_size, speeds, conn_count)
3. Auth: none (`01`), MD5 (`02`), or EC-SRP5 (`03`)
4. TCP: data flows on same connection, 12-byte status messages interleaved every 1s
5. UDP: server sends port number, data on UDP, status exchange stays on TCP

### Client Mode (we connect to MikroTik)

1. Connect to MikroTik:2000
2. Read HELLO, send command
3. Auto-detect auth type from response byte, authenticate
4. Start data transfer with status exchange

### Status Message Format (12 bytes)

```
[0x07][cpu:1][pad:2][seq:4 LE][bytes_received:4 LE]
```

- Byte 0: `0x07` (STATUS_MSG_TYPE)
- Byte 1: `0x80 | cpu_percentage` (MikroTik encoding)
- Bytes 4-7: sequence number (little-endian u32)
- Bytes 8-11: bytes received this interval (little-endian u32)

## Threading Model

All I/O is async via tokio. Per-client:
- **TX task**: sends data packets at target rate
- **RX task**: receives data, counts bytes, extracts status messages (TCP BOTH mode)
- **Status loop**: exchanges 12-byte status messages every 1s, prints bandwidth
- **Status reader** (TCP TX-only): reads server's status messages for remote CPU

Shared state via `Arc<BandwidthState>` with atomic counters — no mutexes.

### BandwidthState Fields

| Field | Type | Purpose |
|-------|------|---------|
| `tx_bytes` | AtomicU64 | Bytes sent this interval (reset by swap) |
| `rx_bytes` | AtomicU64 | Bytes received this interval |
| `tx_speed` | AtomicU32 | Target TX speed (dynamic, from server feedback) |
| `running` | AtomicBool | Test active flag |
| `remote_cpu` | AtomicU8 | Remote peer's CPU (from status messages) |
| `byte_budget` | AtomicU64 | Remaining quota bytes (u64::MAX = unlimited) |
| `total_tx_bytes` | AtomicU64 | Cumulative TX (never reset) |
| `total_rx_bytes` | AtomicU64 | Cumulative RX (never reset) |

## Server Pro Architecture

Optional feature (`--features pro`) providing a multi-user public btest server.

```
Accept → IP check → HELLO → Command → Auth (DB) → Quota check → Budget set → Test
                                                                      ↓
                                                              QuotaEnforcer (parallel)
                                                              - checks every N seconds
                                                              - max_duration timeout
                                                              - sets running=false on exceed
```

**Byte budget**: Before the test starts, `remaining_budget()` computes the minimum remaining quota across all applicable limits. This is stored in `BandwidthState.byte_budget`. Every TX/RX loop checks `spend_budget()` per-packet — when budget hits 0, the test stops immediately. This prevents quota overshoot even on 10+ Gbps links.

**Multi-connection TCP**: MikroTik sends `tcp_conn_count` connections. The first authenticates and registers a session token. Subsequent connections match by token and join. When all connections arrive, the test starts with per-stream TX/RX tasks.

**Web dashboard** (axum):
- `GET /` — landing page with instructions
- `GET /dashboard/{ip}` — per-IP dashboard with Chart.js graph, session table, quota bars
- `GET /api/ip/{ip}/stats` — aggregate stats JSON
- `GET /api/ip/{ip}/sessions` — session list JSON
- `GET /api/ip/{ip}/quota` — quota usage JSON
- `GET /api/ip/{ip}/export` — full export with human-readable fields
- `GET /api/session/{id}/intervals` — per-second throughput data

## CPU Usage Monitoring

A background OS thread samples system CPU every 1 second:

| Platform | Method |
|----------|--------|
| macOS | `host_statistics(HOST_CPU_LOAD_INFO)` |
| Linux | `/proc/stat` aggregate CPU line |
| Android | `/proc/stat` (same as Linux) |
| Windows | `GetSystemTimes()` FFI |
| FreeBSD | `sysctl kern.cp_time` |

Stored in global `AtomicU8`, included in status messages as `0x80 | percentage`.

## Build Targets

| Target | Binary | Notes |
|--------|--------|-------|
| `x86_64-unknown-linux-musl` | btest | Static, zero deps |
| `aarch64-unknown-linux-musl` | btest | RPi 4/5, ARM servers |
| `armv7-unknown-linux-musleabihf` | btest | RPi 3, OpenWrt |
| `x86_64-pc-windows-gnu` | btest.exe | Cross-compiled |
| `aarch64-linux-android` | btest | Termux ARMv8 |
| `armv7-linux-androideabi` | btest | Termux ARMv7 |
| macOS (native) | btest | Apple Silicon + Intel |
| Docker (multi-arch) | image | amd64 + arm64 |

## Key Design Decisions

1. **Tokio async runtime** — all I/O is async, handles hundreds of concurrent connections
2. **Lock-free shared state** — AtomicU64 counters, `swap(0)` reads and resets per interval
3. **Direction bits from server perspective** — `0x01`=server RX, `0x02`=server TX, `0x03`=both
4. **TCP socket half keepalive** — dropping `OwnedWriteHalf` sends FIN, so unused halves are kept alive
5. **Static musl binary** — ~2 MB, zero runtime dependencies
6. **EC-SRP5 with big integer arithmetic** — Curve25519 Weierstrass form via `num-bigint`
7. **Global singletons for syslog/CSV** — `Mutex<Option<...>>` statics, initialized once at startup
8. **Shared BandwidthState for timeout survival** — state created in main(), survives tokio cancellation
9. **Inline byte budget** — per-packet quota check with fast path (u64::MAX = unlimited, returns immediately)
10. **TCP status message scanning** — RX loop detects 12-byte status messages in the data stream by scanning for `0x07` marker byte to extract remote CPU

## Tests

| Suite | Count | What |
|-------|-------|------|
| Unit tests (lib) | 12 | Bandwidth parsing, CPU sampling, auth hash vectors |
| Enforcer tests (pro) | 10 | Budget, quota, duration, flush |
| Integration tests | 8 | Server/client handshake, auth, TCP data |
| EC-SRP5 tests | 6 | Full auth flow, wrong password, UDP bidir |
| Full integration | 23 | All protocols × directions, IPv4/6, CSV, syslog, CPU |
| **Total** | **59** | |
