# btest-rs

A Rust reimplementation of the [MikroTik Bandwidth Test (btest)](https://wiki.mikrotik.com/wiki/Manual:Tools/Bandwidth_Test) protocol. Both server and client modes, compatible with MikroTik RouterOS devices.

## Based on

This project is a clean-room Rust reimplementation based on the protocol reverse-engineering work done by **Alex Samorukov** in [btest-opensource](https://github.com/samm-git/btest-opensource). The original C implementation and protocol documentation were invaluable in making this project possible. Full credit to Alex and all contributors to that project.

The original `btest-opensource` project is included as a git submodule for reference and protocol documentation.

## Why Rust?

- **Single static binary** - 2 MB, zero dependencies, runs anywhere
- **Cross-platform** - macOS, Linux (x86_64, ARM64), Docker
- **Async I/O** - tokio-based, handles many concurrent connections efficiently
- **Memory safe** - no buffer overflows, no use-after-free, no data races
- **Easy deployment** - `scp` one file, done. Or use the systemd installer.

## Performance

Tested over WiFi 6E (MikroTik RouterOS <-> macOS):

| Mode | Protocol | Speed |
|------|----------|-------|
| Server RX (1 conn) | UDP | **1.05 Gbps** |
| Client TCP download | TCP | **530 Mbps** |
| Client TCP upload | TCP | **840 Mbps** |
| Client UDP download | UDP | **433 Mbps** |
| Client TCP bidirectional | TCP | **264/264 Mbps** |
| Server bidirectional | UDP | **280/393 Mbps** |

## Installation

### Pre-built binary

```bash
# Build for Linux x86_64 from macOS (requires Docker)
scripts/build-linux.sh

# Copy to server
scp dist/btest root@yourserver:/usr/local/bin/btest
```

### From source

```bash
cargo install --path .
```

### Docker

```bash
docker compose up -d    # Server on port 2000
```

### systemd service

```bash
# On the target Linux server:
sudo ./scripts/install-service.sh
sudo ./scripts/install-service.sh --auth-user admin --auth-pass secret
```

## Usage

### Server mode

MikroTik devices connect to this server to run bandwidth tests.

```bash
# Basic server (no auth)
btest -s

# With authentication
btest -s -a admin -p password

# Custom port with verbose logging
btest -s -P 2000 -v
```

### Client mode

Connect to a MikroTik device's built-in btest server.

```bash
# TCP download test
btest -c 192.168.88.1 -r

# TCP upload test
btest -c 192.168.88.1 -t

# Bidirectional
btest -c 192.168.88.1 -t -r

# UDP with bandwidth limit
btest -c 192.168.88.1 -r -u -b 100M

# With authentication
btest -c 192.168.88.1 -r -a admin -p password
```

### Debug logging

```bash
btest -s -v      # info + debug
btest -s -vv     # info + debug + trace (hex dumps of status exchange)
```

## MikroTik Setup

### Enable btest server on MikroTik (for client mode)

```
/tool/bandwidth-server set enabled=yes
```

### Run btest from MikroTik (connecting to our server)

**Important: Set Connection Count to 1** — multi-connection mode is not supported.

```
/tool/bandwidth-test address=<server-ip> direction=both protocol=udp user=admin password=password connection-count=1
```

## Protocol

The MikroTik btest protocol uses:
- **TCP port 2000** for control (handshake, auth, status exchange)
- **UDP ports 2001+** for data transfer
- **MD5 challenge-response** authentication (RouterOS < 6.43)
- **1-second status interval** with dynamic speed adjustment

See the [original protocol documentation](btest-opensource/README.md) for wire-format details.

## Authentication

Both MD5 (legacy) and EC-SRP5 (RouterOS >= 6.43) authentication are supported:

```bash
# Server with MD5 auth (legacy clients)
btest -s -a admin -p password

# Server with EC-SRP5 auth (modern RouterOS clients)
btest -s -a admin -p password --ecsrp5

# Client auto-detects auth type
btest -c 192.168.88.1 -r -a admin -p password
```

## Known Limitations

- **IPv6 support is experimental** (`--listen6`). TCP over IPv6 works fully. UDP over IPv6 has issues on macOS due to kernel ENOBUFS limitations with `send_to()`. On Linux, IPv6 UDP works fine. IPv6 is disabled by default.
- **Multi-connection UDP** is supported. MikroTik's multi-connection mode sends from multiple source ports which are all accepted by the server.

## Testing

```bash
cargo test                           # Unit + integration tests
scripts/test-local.sh                # Loopback self-test
scripts/test-mikrotik.sh <ip>        # Test against MikroTik device
scripts/test-docker.sh               # Docker container test
```

## Credits

- **[btest-opensource](https://github.com/samm-git/btest-opensource)** by [Alex Samorukov](https://github.com/samm-git) — Original C implementation and protocol reverse-engineering. Licensed under MIT.
- **[Margin Research](https://github.com/MarginResearch/mikrotik_authentication)** — EC-SRP5 authentication reverse-engineering (Curve25519 Weierstrass, SRP key exchange). Licensed under Apache 2.0.
- **MikroTik** — Creator of the bandwidth test protocol and RouterOS.

## License

MIT License - see [LICENSE](LICENSE).

This project is derived from [btest-opensource](https://github.com/samm-git/btest-opensource) (MIT License, Copyright 2016 Alex Samorukov). The original license and copyright notice are preserved as required.
