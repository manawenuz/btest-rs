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

### Basic Server

```bash
btest -s
```

Listens on TCP port 2000 (default). Any MikroTik device can connect without authentication.

### Server with Authentication

```bash
btest -s -a admin -p mysecretpassword
```

MikroTik devices must provide matching credentials. Uses MD5 challenge-response authentication.

### Custom Port

```bash
btest -s -P 3000
```

### Verbose/Debug Output

```bash
btest -s -v      # Show connection info and debug messages
btest -s -vv     # Show hex dumps of status exchange (for debugging)
```

### MikroTik Configuration (connecting to our server)

On the MikroTik device (WinBox or CLI):

```
# CLI
/tool/bandwidth-test address=<server-ip> direction=both protocol=udp user=admin password=mysecretpassword

# For best results, use 1 connection
/tool/bandwidth-test address=<server-ip> direction=both protocol=udp connection-count=1
```

Or via WinBox: **Tools → Bandwidth Test**, enter server address, credentials, and click Start.

## Client Mode

Connect to a MikroTik device's built-in bandwidth test server.

### Prerequisites

Enable btest server on MikroTik:
```
/tool/bandwidth-server set enabled=yes
```

**Note**: If the MikroTik uses RouterOS >= 6.43 with authentication enabled, you'll need to either disable auth or use credentials. EC-SRP5 auth is not yet supported; MD5 auth works on older RouterOS versions.

### Download Test (receive)

```bash
btest -c 192.168.88.1 -r
```

Measures download speed from MikroTik to your machine.

### Upload Test (transmit)

```bash
btest -c 192.168.88.1 -t
```

Measures upload speed from your machine to MikroTik.

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

### Bandwidth Limiting

```bash
btest -c 192.168.88.1 -r -b 100M     # Limit to 100 Mbps
btest -c 192.168.88.1 -t -b 1G       # Limit to 1 Gbps
btest -c 192.168.88.1 -r -b 500K     # Limit to 500 Kbps
```

### NAT Traversal

If you're behind NAT and need to receive UDP data:

```bash
btest -c 192.168.88.1 -r -u -n
```

The `-n` flag sends a probe packet to open the NAT firewall hole.

### With Authentication

```bash
btest -c 192.168.88.1 -r -a admin -p password
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
| `TX` | Data we sent (upload) |
| `RX` | Data we received (download) |
| `Mbps` | Megabits per second |
| `bytes` | Raw bytes transferred in this interval |
| `lost: N` | UDP packets lost (UDP mode only) |

## CLI Reference

```
btest-rs — MikroTik Bandwidth Test server & client in Rust

Usage: btest [OPTIONS]

Options:
  -s, --server              Run in server mode
  -c, --client <HOST>       Run in client mode, connect to HOST
  -t, --transmit            Client: upload test
  -r, --receive             Client: download test
  -u, --udp                 Use UDP instead of TCP
  -b, --bandwidth <BW>      Bandwidth limit (e.g., 100M, 1G, 500K)
  -P, --port <PORT>         Port number [default: 2000]
  -a, --authuser <USER>     Authentication username
  -p, --authpass <PASS>     Authentication password
  -n, --nat                 NAT traversal mode
  -v, --verbose             Increase log verbosity (-v, -vv)
  -h, --help                Show help
  -V, --version             Show version
```

## Tips

- **Use 1 connection** when MikroTik connects to your server. Multi-connection mode causes MikroTik's per-connection speed adaptation to throttle.
- **TCP mode** generally gives more stable results than UDP due to TCP flow control.
- **UDP mode** is better for measuring raw link capacity without TCP overhead.
- **First interval** may show higher or lower numbers as the connection stabilizes. Look at intervals 3+ for steady-state throughput.
- **WiFi testing**: bidirectional tests on WiFi will show lower per-direction speeds because WiFi is half-duplex at the MAC layer.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| `EC-SRP5 authentication not supported` | Disable auth on MikroTik btest server, or use older RouterOS |
| `Connection refused` | Check port 2000 is open, firewall allows it |
| Server shows 0 RX | Check MikroTik is actually sending (direction setting) |
| Speed drops over time (server mode) | MikroTik client behavior — use 1 connection, or use our client mode instead |
| UDP `lost` packets high | Network congestion or MTU issues, try reducing bandwidth with `-b` |
