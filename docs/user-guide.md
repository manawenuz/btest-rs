# btest-rs User Guide

## Quick Start

```bash
# Server mode (MikroTik connects to you)
btest -s

# Client mode (you connect to MikroTik)
btest -c 192.168.88.1 -r
```

## Server Mode

Run btest-rs as a server and let MikroTik devices connect for bandwidth testing.

### Basic Server (No Authentication)

```bash
btest -s
```

Listens on all IPv4 interfaces, TCP port 2000. Any MikroTik device can connect without credentials.

### Server with MD5 Authentication

```bash
btest -s -a admin -p mysecretpassword
```

Requires connecting devices to provide matching credentials. Uses MD5 double-hash challenge-response authentication, compatible with RouterOS versions before 6.43.

### Server with EC-SRP5 Authentication

```bash
btest -s -a admin -p mysecretpassword --ecsrp5
```

Advertises EC-SRP5 (Curve25519 Weierstrass) authentication to connecting clients. Required for RouterOS >= 6.43 devices that use the modern authentication protocol.

### Custom Port

```bash
btest -s -P 3000
```

### Custom Listen Address

```bash
# Listen only on a specific interface
btest -s --listen 10.0.0.1

# Disable IPv4, listen only on IPv6
btest -s --listen none --listen6

# Listen on both IPv4 and IPv6
btest -s --listen6
```

### IPv6 Listener (Experimental)

```bash
# IPv6 on default address (::)
btest -s --listen6

# IPv6 on a specific address
btest -s --listen6 fd00::1
```

TCP over IPv6 works fully on all platforms. UDP over IPv6 has issues on macOS due to kernel ENOBUFS limitations with `send_to()`. On Linux, IPv6 UDP works correctly.

### Syslog Integration

```bash
btest -s --syslog 192.168.1.1:514
```

Sends structured log events to a remote syslog server via UDP (RFC 3164 / BSD syslog format, facility local0). Events include:

- `AUTH_SUCCESS` -- successful authentication with peer address, username, and auth type
- `AUTH_FAILURE` -- failed authentication with peer address, username, auth type, and reason
- `TEST_START` -- test initiated with peer address, protocol, direction, and connection count
- `TEST_END` -- test completed with peer address, protocol, direction, duration, average speeds, bytes transferred, and lost packets

### CSV Output

```bash
btest -s --csv /var/log/btest-results.csv
```

Appends a row for each completed test to the specified CSV file. Creates the file with headers if it does not exist. CSV columns:

```
timestamp,host,port,protocol,direction,duration_s,tx_avg_mbps,rx_avg_mbps,tx_bytes,rx_bytes,lost_packets,auth_type
```

### Quiet Mode

```bash
btest -s --csv /var/log/btest.csv -q
```

Suppresses per-second terminal output. Useful when running as a background service with CSV or syslog logging only.

### Verbose/Debug Output

```bash
btest -s -v       # Debug messages (connection lifecycle, auth steps)
btest -s -vv      # Trace messages (hex dumps of status exchange)
btest -s -vvv     # Maximum verbosity
```

### Combined Example

```bash
btest -s -a admin -p secret --ecsrp5 --syslog 10.0.0.1:514 --csv /var/log/btest.csv -v
```

This runs a server with EC-SRP5 authentication, sends events to syslog, logs results to CSV, and prints debug output to the terminal.

### MikroTik Configuration (Connecting to Our Server)

On the MikroTik device (WinBox or CLI):

```
/tool/bandwidth-test address=<server-ip> direction=both protocol=udp \
    user=admin password=mysecretpassword
```

Or via WinBox: **Tools > Bandwidth Test**, enter the server address and credentials, and click Start.

## Client Mode

Connect to a MikroTik device's built-in bandwidth test server.

### Prerequisites

Enable the btest server on the MikroTik device:

```
/tool/bandwidth-server set enabled=yes
```

### Download Test (Receive)

```bash
btest -c 192.168.88.1 -r
```

Measures download speed from the MikroTik device to your machine. The server transmits, the client receives.

### Upload Test (Transmit)

```bash
btest -c 192.168.88.1 -t
```

Measures upload speed from your machine to the MikroTik device. The client transmits, the server receives.

### Bidirectional Test

```bash
btest -c 192.168.88.1 -t -r
```

Tests both directions simultaneously.

### UDP Mode

```bash
btest -c 192.168.88.1 -r -u          # UDP download
btest -c 192.168.88.1 -t -u          # UDP upload
btest -c 192.168.88.1 -t -r -u       # UDP bidirectional
```

UDP mode uses separate data ports (2001+ on the server side, 2257+ on the client side) and exchanges status messages every second over the TCP control channel.

### Bandwidth Limiting

```bash
btest -c 192.168.88.1 -r -b 100M     # Limit to 100 Mbps
btest -c 192.168.88.1 -t -b 1G       # Limit to 1 Gbps
btest -c 192.168.88.1 -r -b 500K     # Limit to 500 Kbps
```

Suffixes: `K` (kilobits/sec), `M` (megabits/sec), `G` (gigabits/sec). Values are in bits per second.

### With Authentication

```bash
btest -c 192.168.88.1 -r -a admin -p password
```

The client auto-detects the authentication type (MD5 or EC-SRP5) from the server's response and handles it accordingly.

### NAT Traversal

```bash
btest -c 192.168.88.1 -r -u -n
```

The `-n` flag sends an empty UDP probe packet before starting the receive thread. This opens a hole in NAT firewalls so the server's UDP data packets can reach the client.

### Timed Tests

```bash
btest -c 192.168.88.1 -r -d 30       # Run for 30 seconds, then stop
btest -c 192.168.88.1 -t -r -d 60    # 60-second bidirectional test
```

The default duration is 0 (unlimited). When the duration expires, the client exits cleanly.

### CSV Output (Client Mode)

```bash
btest -c 192.168.88.1 -r -d 30 --csv results.csv
```

Appends a summary row after the test completes with the host, port, protocol, direction, duration, and auth type.

### Quiet Mode (Client)

```bash
btest -c 192.168.88.1 -r -d 10 --csv results.csv -q
```

Suppresses per-second bandwidth output to the terminal. Useful for scripted or automated testing where only the CSV file matters.

### Custom Port

```bash
btest -c 192.168.88.1 -r -P 3000
```

## Reading the Output

```
[   1]  TX  264.50 Mbps (33062912 bytes)
[   2]  TX  263.98 Mbps (32997376 bytes)
[   2]  RX  263.98 Mbps (32997012 bytes)
[   3]  RX  430.51 Mbps (53813376 bytes)  lost: 5
```

| Field | Meaning |
|-------|---------|
| `[  N]` | Interval number (1 per second) |
| `TX` | Data sent (upload from your perspective) |
| `RX` | Data received (download from your perspective) |
| `Mbps` | Megabits per second |
| `bytes` | Raw bytes transferred in this interval |
| `lost: N` | UDP packets lost in this interval (UDP mode only) |

## Complete CLI Reference

```
btest-rs -- MikroTik Bandwidth Test server & client in Rust

Usage: btest [OPTIONS]

Options:
  -s, --server
          Run in server mode. Listens for incoming connections from MikroTik
          devices or other btest clients. Conflicts with -c.

  -c, --client <HOST>
          Run in client mode, connecting to the specified host. The host can be
          an IPv4 address, IPv6 address, or hostname. Conflicts with -s.

  -t, --transmit
          Client transmits data (upload test). Tells the server to receive.
          Can be combined with -r for bidirectional testing.

  -r, --receive
          Client receives data (download test). Tells the server to transmit.
          Can be combined with -t for bidirectional testing.

  -u, --udp
          Use UDP instead of TCP for the data transfer. UDP uses separate data
          ports (2001+ server side, 2257+ client side) and exchanges status
          messages over the TCP control channel every second.

  -b, --bandwidth <BW>
          Target bandwidth limit for the test. Accepts suffixes: K (kilobits),
          M (megabits), G (gigabits). Examples: 100M, 1G, 500K. Default is 0
          (unlimited).

  -P, --port <PORT>
          TCP port to listen on (server mode) or connect to (client mode).
          [default: 2000]

      --listen <ADDR>
          IPv4 address to bind the server listener to. Use "none" to disable
          IPv4 listening entirely (useful with --listen6 for IPv6-only mode).
          [default: 0.0.0.0]

      --listen6 [<ADDR>]
          Enable the IPv6 listener. If no address is given, binds to [::].
          Experimental: TCP over IPv6 works fully on all platforms. UDP over
          IPv6 has issues on macOS due to kernel ENOBUFS limitations.

  -a, --authuser <USER>
          Authentication username. In server mode, clients must provide this
          username. In client mode, this is sent to the server.

  -p, --authpass <PASS>
          Authentication password. In server mode, clients must provide a
          matching password. In client mode, this is used to authenticate.

      --ecsrp5
          Use EC-SRP5 authentication (Curve25519 Weierstrass). In server mode,
          this advertises EC-SRP5 instead of MD5 to connecting clients.
          Required for RouterOS >= 6.43. In client mode, auth type is
          auto-detected and this flag is not needed.

  -n, --nat
          NAT traversal mode. Sends an empty UDP probe packet to the server
          before starting the receive thread, opening a hole in NAT firewalls.
          Only relevant for UDP receive tests behind NAT.

  -d, --duration <SECS>
          Test duration in seconds (client mode only). The client exits cleanly
          after the specified time. A value of 0 means unlimited (run until
          interrupted with Ctrl-C). [default: 0]

      --csv <FILE>
          Output test results to a CSV file. Appends a row per completed test.
          Creates the file with a header row if it does not exist. Columns:
          timestamp, host, port, protocol, direction, duration_s, tx_avg_mbps,
          rx_avg_mbps, tx_bytes, rx_bytes, lost_packets, auth_type.

  -q, --quiet
          Suppress per-second bandwidth output to the terminal. Useful in
          combination with --csv for machine-readable-only output, or when
          running as a background service.

      --syslog <HOST:PORT>
          Send structured log events to a remote syslog server via UDP. Uses
          RFC 3164 (BSD syslog) format with facility local0. Events include
          AUTH_SUCCESS, AUTH_FAILURE, TEST_START, and TEST_END with detailed
          metadata. Example: --syslog 192.168.1.1:514

  -v, --verbose...
          Increase log verbosity. Can be repeated:
            -v    debug messages (connection lifecycle, auth steps)
            -vv   trace messages (hex dumps of protocol exchange)
            -vvv  maximum verbosity

  -h, --help
          Print help information

  -V, --version
          Print version information
```

## Tips

- **TCP mode** generally gives more stable results than UDP due to TCP flow control.
- **UDP mode** is better for measuring raw link capacity without TCP overhead.
- **First interval** may show higher or lower numbers as the connection stabilizes. Look at intervals 3+ for steady-state throughput.
- **WiFi testing**: bidirectional tests on WiFi will show lower per-direction speeds because WiFi is half-duplex at the MAC layer.
- **Bandwidth limiting** applies to the direction you specify. In bidirectional mode with `-b 100M`, both directions are limited to 100 Mbps each.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| Connection refused | Check that port 2000 is open and the server is running |
| Auth failure with EC-SRP5 | Ensure `--ecsrp5` is set on the server if the MikroTik client uses RouterOS >= 6.43 |
| Auth failure with MD5 | Verify username and password match exactly (case-sensitive) |
| Server shows 0 RX | Check that the MikroTik direction setting includes sending to the server |
| Very low UDP speed | Network congestion or MTU issues; try reducing bandwidth with `-b` |
| IPv6 UDP fails on macOS | Known macOS kernel limitation; use Linux for IPv6 UDP tests |
| Syslog messages not arriving | Verify the syslog server address and port, and check firewall rules for UDP 514 |
| CSV file not created | Check write permissions on the directory; the file is created on first use |
