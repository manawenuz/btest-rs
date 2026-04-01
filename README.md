# btest-rs

A Rust reimplementation of the [MikroTik Bandwidth Test (btest)](https://wiki.mikrotik.com/wiki/Manual:Tools/Bandwidth_Test) protocol. Both server and client modes, fully compatible with MikroTik RouterOS devices.

## Features

- **Full protocol support** -- TCP and UDP data transfer, IPv4 and IPv6
- **EC-SRP5 authentication** -- modern RouterOS >= 6.43 Curve25519-based auth (server and client)
- **MD5 authentication** -- legacy RouterOS < 6.43 challenge-response auth
- **Multi-connection support** -- handles MikroTik's multi-connection UDP mode
- **Bidirectional testing** -- simultaneous upload and download
- **Syslog logging** -- send structured events (auth, test start/end) to a remote syslog server
- **CSV output** -- append machine-readable test results to a CSV file
- **CPU usage monitoring** -- local and remote CPU shown per interval, warning at >70%
- **Timed tests** -- `--duration` flag to automatically stop after N seconds
- **Quiet mode** -- suppress terminal output for scripted/automated use
- **NAT traversal** -- probe packet to open firewall holes for UDP receive
- **Single static binary** -- ~2 MB, zero runtime dependencies (musl build)
- **Cross-platform** -- macOS, Linux (x86_64, ARM64), Docker
- **Async I/O** -- tokio-based, handles many concurrent connections efficiently

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

On wired gigabit links, expect line-rate performance in both TCP and UDP modes.

## Installation

### From source

```bash
cargo install --path .
```

### Pre-built binaries

Download from [releases](https://git.manko.yoga/manawenuz/btest-rs/releases) or [GitHub releases](https://github.com/manawenuz/btest-rs/releases):

```bash
# Linux x86_64
curl -L <release-url>/btest-linux-x86_64.tar.gz | tar xz
sudo mv btest /usr/local/bin/

# Raspberry Pi 4/5 (64-bit OS)
curl -L <release-url>/btest-linux-aarch64.tar.gz | tar xz
sudo mv btest /usr/local/bin/

# Raspberry Pi 3/Zero 2 (32-bit OS)
curl -L <release-url>/btest-linux-armv7.tar.gz | tar xz
sudo mv btest /usr/local/bin/

# Windows
# Download btest-windows-x86_64.zip from releases
```

### Raspberry Pi

The static musl binaries run on any Raspberry Pi without dependencies:

```bash
# On the Pi — detect architecture and install
ARCH=$(uname -m)
case $ARCH in
  aarch64) FILE=btest-linux-aarch64.tar.gz ;;
  armv7l)  FILE=btest-linux-armv7.tar.gz ;;
  *)       echo "Unsupported: $ARCH"; exit 1 ;;
esac

curl -LO "https://github.com/manawenuz/btest-rs/releases/latest/download/$FILE"
tar xzf "$FILE"
sudo mv btest /usr/local/bin/
rm "$FILE"

# Run as server
btest -s -a admin -p password --ecsrp5

# Or install as systemd service
curl -LO https://raw.githubusercontent.com/manawenuz/btest-rs/main/scripts/install-service.sh
sudo bash install-service.sh --auth-user admin --auth-pass password
```

### Docker

```bash
docker compose up -d
```

See [docs/docker.md](docs/docker.md) for full Docker and deployment options.

### systemd service

```bash
sudo ./scripts/install-service.sh
sudo ./scripts/install-service.sh --auth-user admin --auth-pass secret
sudo ./scripts/install-service.sh --auth-user admin --auth-pass secret --port 2000
```

The installer creates a dedicated `btest` system user, installs a hardened systemd unit, and enables the service.

## Quick Start

### Server mode

MikroTik devices connect to this server to run bandwidth tests.

```bash
# No authentication
btest -s

# MD5 authentication (legacy RouterOS)
btest -s -a admin -p password

# EC-SRP5 authentication (RouterOS >= 6.43)
btest -s -a admin -p password --ecsrp5

# Custom port, verbose logging
btest -s -P 3000 -v

# With syslog and CSV logging
btest -s -a admin -p password --syslog 192.168.1.1:514 --csv /var/log/btest.csv
```

### Client mode

Connect to a MikroTik device's built-in btest server.

```bash
# TCP download test
btest -c 192.168.88.1 -r

# TCP upload test
btest -c 192.168.88.1 -t

# Bidirectional TCP
btest -c 192.168.88.1 -t -r

# UDP download with bandwidth limit
btest -c 192.168.88.1 -r -u -b 100M

# With authentication
btest -c 192.168.88.1 -r -a admin -p password

# Timed test (30 seconds), results to CSV
btest -c 192.168.88.1 -r -d 30 --csv results.csv

# Quiet mode (no terminal output)
btest -c 192.168.88.1 -r -d 10 --csv results.csv -q

# UDP through NAT
btest -c 192.168.88.1 -r -u -n
```

### Debug logging

```bash
btest -s -v       # debug messages
btest -s -vv      # trace messages (hex dumps of status exchange)
btest -s -vvv     # maximum verbosity
```

## CLI Reference

```
Usage: btest [OPTIONS]

Options:
  -s, --server                Run in server mode
  -c, --client <HOST>         Run in client mode, connect to HOST
  -t, --transmit              Client transmits data (upload test)
  -r, --receive               Client receives data (download test)
  -u, --udp                   Use UDP instead of TCP
  -b, --bandwidth <BW>        Target bandwidth limit (e.g., 100M, 1G, 500K)
  -P, --port <PORT>           Listen/connect port [default: 2000]
      --listen <ADDR>         IPv4 listen address [default: 0.0.0.0] (use "none" to disable)
      --listen6 [<ADDR>]      Enable IPv6 listener [default: ::] (experimental)
  -a, --authuser <USER>       Authentication username
  -p, --authpass <PASS>       Authentication password
      --ecsrp5                Use EC-SRP5 authentication (RouterOS >= 6.43)
  -n, --nat                   NAT traversal mode (send UDP probe packet)
  -d, --duration <SECS>       Test duration in seconds (client mode, 0=unlimited) [default: 0]
      --csv <FILE>            Output results to CSV file (appends if file exists)
  -q, --quiet                 Suppress terminal output (use with --csv)
      --syslog <HOST:PORT>    Send logs to remote syslog server (UDP, RFC 3164)
  -v, --verbose               Increase log verbosity (-v, -vv, -vvv)
  -h, --help                  Show help
  -V, --version               Show version
```

## MikroTik Configuration

### Enable btest server on MikroTik (for client mode)

```
/tool/bandwidth-server set enabled=yes
```

### Run btest from MikroTik (connecting to our server)

```
/tool/bandwidth-test address=<server-ip> direction=both protocol=udp \
    user=admin password=password
```

## Protocol

The MikroTik btest protocol uses:

- **TCP port 2000** for control (handshake, authentication, status exchange)
- **UDP ports 2001+** for data transfer (server side)
- **UDP ports 2257+** for data transfer (client side, offset +256)
- **MD5 double-hash challenge-response** authentication (RouterOS < 6.43)
- **EC-SRP5 Curve25519 Weierstrass** authentication (RouterOS >= 6.43)
- **1-second status interval** with dynamic speed adjustment

See [docs/protocol.md](docs/protocol.md) for the full wire-format specification.

## Authentication

Both legacy and modern MikroTik authentication schemes are supported:

| Scheme | RouterOS Version | Flag |
|--------|-----------------|------|
| None | Any | (no flags) |
| MD5 challenge-response | < 6.43 | `-a USER -p PASS` |
| EC-SRP5 (Curve25519) | >= 6.43 | `-a USER -p PASS --ecsrp5` |

In server mode, `--ecsrp5` advertises EC-SRP5 to connecting clients. Without it, MD5 is advertised. In client mode, the authentication type is auto-detected from the server's response.

## Known Issues

See [KNOWN_ISSUES.md](KNOWN_ISSUES.md) for the full list including:

- **IPv6 UDP on macOS** — server TX hits ENOBUFS, use IPv4 or deploy on Linux
- **macOS UDP send buffer** — first 2-3 seconds unreliable on unlimited speed tests
- **Windows binaries** — cross-compiled but untested
- **IPv6 UDP on Linux** — untested, likely works fine

Contributions and bug reports welcome:
- https://github.com/manawenuz/btest-rs/issues
- https://git.manko.yoga/manawenuz/btest-rs/issues

## Documentation

- [User Guide](docs/user-guide.md) -- complete CLI reference with examples for every mode
- [Architecture](docs/architecture.md) -- module structure, threading model, design decisions
- [Protocol Specification](docs/protocol.md) -- wire format, authentication, status exchange
- [Docker & Deployment](docs/docker.md) -- Docker, Docker Compose, systemd, firewall rules
- [EC-SRP5 Research](docs/ecsrp5-research.md) -- reverse-engineering notes and cryptographic details
- [Man Page](docs/man/btest.1) -- Unix manual page (install to `/usr/share/man/man1/`)

## Testing

```bash
cargo test                           # Unit + integration tests
scripts/test-local.sh                # Loopback self-test
scripts/test-mikrotik.sh <ip>        # Test against MikroTik device
scripts/test-docker.sh               # Docker container test
```

## Credits

- **[btest-opensource](https://github.com/samm-git/btest-opensource)** by [Alex Samorukov](https://github.com/samm-git) -- original C implementation and protocol reverse-engineering. Licensed under **MIT**.
- **[Margin Research](https://github.com/MarginResearch/mikrotik_authentication)** -- EC-SRP5 authentication reverse-engineering (Curve25519 Weierstrass, SRP key exchange). Licensed under **Apache 2.0**.
- **MikroTik** -- creator of the bandwidth test protocol and RouterOS.

## License

MIT License -- see [LICENSE](LICENSE).

This project is derived from [btest-opensource](https://github.com/samm-git/btest-opensource) (MIT License, Copyright 2016 Alex Samorukov). The EC-SRP5 implementation is based on research by [Margin Research](https://github.com/MarginResearch/mikrotik_authentication) (Apache License 2.0). Original license and copyright notices are preserved as required.
