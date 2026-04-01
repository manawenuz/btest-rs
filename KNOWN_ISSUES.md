# Known Issues

This document tracks known limitations, bugs, and platform-specific issues in btest-rs. If you encounter an issue not listed here, please report it at: **https://git.manko.yoga/manawenuz/btest-rs/issues**

## IPv6 UDP on macOS (Server Mode)

**Severity:** High
**Affects:** macOS only (server mode, UDP, IPv6)
**Status:** Open

When running as a server on macOS and a MikroTik client connects over IPv6 UDP, the server's UDP transmit hits `ENOBUFS` (error 55 — "No buffer space available") repeatedly. This causes:

- Direction "receive" (server TX): intermittent packet bursts with gaps, MikroTik shows unstable or low speed
- Direction "send" (server RX): works, but speed drops over time due to MikroTik's speed adaptation receiving irregular status feedback
- Direction "both": TX side severely degraded

**Root cause:** macOS kernel returns `ENOBUFS` on IPv6 `send_to()` much more aggressively than IPv4 due to smaller interface output queues and per-packet NDP overhead. Connected sockets (`send()`) perform better than unconnected (`send_to()`), but still hit limits under high throughput.

**Workaround:** Use IPv4 for UDP tests on macOS, or deploy the server on Linux where IPv6 UDP works correctly.

**Not affected:**
- IPv4 UDP (all directions, all platforms)
- IPv6 TCP (all directions, all platforms)
- Client mode over IPv6 (connecting TO a MikroTik server works fine at 600+ Mbps)

## IPv6 UDP — Not Tested on Linux

**Severity:** Unknown
**Affects:** Linux server, IPv6, UDP
**Status:** Untested

IPv6 UDP in server mode has not been thoroughly tested on Linux. The macOS ENOBUFS issue is kernel-specific and likely does not exist on Linux (which has much better IPv6 UDP buffer management). Testing and reports welcome.

## macOS UDP Send Buffer Saturation

**Severity:** Medium
**Affects:** macOS (client and server, IPv4 and IPv6, UDP)
**Status:** Mitigated

On macOS, when sending UDP at unlimited speed, the kernel buffer fills quickly and returns `ENOBUFS`. The adaptive backoff mechanism (200μs → 10ms) mitigates this, but the first few seconds of a test may show:

- Interval 1: high burst (40-300 Mbps depending on conditions)
- Interval 2: 0 bps (buffer full, backoff in effect)
- Interval 3+: gradually recovers to steady state

This causes the first 2-3 seconds of UDP tests to be unreliable on macOS. On Linux, this issue does not occur.

**Workaround:** Ignore the first few seconds of results, or use TCP mode which does not have this issue.

## Windows Binaries Not Tested

**Severity:** Unknown
**Affects:** Windows x86_64
**Status:** Untested

Windows binaries are cross-compiled from Linux using `gcc-mingw-w64` in CI. They have never been tested on actual Windows systems. Issues may include:

- Socket behavior differences (Winsock vs BSD sockets)
- IPv6 dual-stack handling
- Path separator issues in CSV output
- Console output encoding

**Help wanted:** If you test on Windows, please report your findings.

## EC-SRP5 Server Authentication — Occasional Failure

**Severity:** Low
**Affects:** Server mode with `--ecsrp5`
**Status:** Mostly fixed

EC-SRP5 server authentication occasionally fails with "client proof mismatch". This was largely fixed by storing the correct gamma parity from key derivation, but edge cases may still exist with certain salt/password combinations due to the Curve25519 Weierstrass arithmetic.

**Workaround:** Retry the connection. If it fails consistently, restart the server (which regenerates the salt).

## MikroTik Speed Adaptation Staircase (Server RX, UDP)

**Severity:** Low
**Affects:** Server mode, UDP, direction "send" (MikroTik sends to us)
**Status:** MikroTik client behavior

When MikroTik connects as a client and sends data (direction "send"), the speed may gradually decrease in a staircase pattern over 30-60 seconds. This is caused by MikroTik's client-side speed adaptation algorithm, not by our server.

The original C btest-opensource server exhibits the same behavior. Single-connection mode (`connection-count=1`) provides the best results.

## TCP Multi-Connection Bandwidth Reporting

**Severity:** Low
**Affects:** Server mode, TCP, `connection-count > 1`
**Status:** Open

With TCP multi-connection, the server correctly handles all connections and data flows, but bandwidth is only measured on the primary connection's status loop. MikroTik may show lower-than-actual speeds because status messages are not distributed across all connections.

## Bandwidth Limit (`-b`) Not Fully Effective

**Severity:** Low
**Affects:** Client mode, `-b` flag
**Status:** Open

The `-b` bandwidth limit flag does not reliably cap speed. The `calc_send_interval` function computes the inter-packet delay correctly, but tokio's timer resolution and task scheduling can cause actual throughput to exceed the specified limit, especially for high bandwidth values.

---

## Reporting Issues

Found a bug or unexpected behavior? Please report it:

- **Issue tracker:** https://git.manko.yoga/manawenuz/btest-rs/issues
- **Include:** OS/platform, btest-rs version (`btest --version`), MikroTik RouterOS version, protocol (TCP/UDP), direction, connection count, and the full command line used.
- **Packet captures:** If possible, attach a tcpdump/pcap capture. Use: `sudo tcpdump -i <interface> -w capture.pcap -s 200 'host <mikrotik_ip> and (port 2000 or portrange 2001-2356)'`
- **Debug logs:** Run with `-vv` to get hex-level status exchange dumps.

## Platform Test Matrix

| Platform | TCP4 | UDP4 | TCP6 | UDP6 | Notes |
|----------|------|------|------|------|-------|
| macOS (ARM64) | Pass | Pass* | Pass | Fail** | *UDP send buffer saturation on first seconds |
| macOS (x86_64) | Untested | Untested | Untested | Untested | |
| Linux (x86_64) | Pass | Pass | Pass | Untested | Deployed on Ubuntu 24.04 |
| Linux (aarch64) | Untested | Untested | Untested | Untested | RPi builds available |
| Linux (armv7) | Untested | Untested | Untested | Untested | RPi builds available |
| Windows (x86_64) | Untested | Untested | Untested | Untested | Cross-compiled, never tested |

**Pass** = verified against MikroTik RouterOS 7.x
**Fail** = known issue documented above
**Untested** = builds available but not verified
