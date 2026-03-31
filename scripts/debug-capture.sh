#!/usr/bin/env bash
# Capture btest traffic for debugging multi-connection issues.
#
# Usage:
#   # Terminal 1: Start capture
#   sudo ./scripts/debug-capture.sh capture <interface> [mikrotik_ip]
#
#   # Terminal 2: Run server or client
#   ./target/release/btest -s -a admin -p password -vv
#
#   # Terminal 1: Stop with Ctrl+C, then analyze
#   ./scripts/debug-capture.sh analyze
set -euo pipefail

cd "$(dirname "$0")/.."

CMD="${1:?Usage: $0 <capture|analyze> [interface] [mikrotik_ip]}"

PCAP_FILE="dist/btest-debug.pcap"
mkdir -p dist

case "$CMD" in
    capture)
        IFACE="${2:?Specify interface (e.g., en0, eth0)}"
        MK_IP="${3:-}"

        FILTER="port 2000 or portrange 2001-2100 or portrange 2257-2356"
        if [[ -n "$MK_IP" ]]; then
            FILTER="host $MK_IP and ($FILTER)"
        fi

        echo "Capturing btest traffic on $IFACE..."
        echo "Filter: $FILTER"
        echo "Output: $PCAP_FILE"
        echo "Press Ctrl+C to stop"
        echo ""
        tcpdump -i "$IFACE" -w "$PCAP_FILE" -s 128 "$FILTER"
        ;;

    analyze)
        if [[ ! -f "$PCAP_FILE" ]]; then
            echo "No capture file found at $PCAP_FILE"
            echo "Run: sudo $0 capture <interface> first"
            exit 1
        fi

        echo "=== TCP Control Channel (port 2000) ==="
        echo ""
        echo "--- Connection summary ---"
        tcpdump -r "$PCAP_FILE" -n 'tcp port 2000 and (tcp[tcpflags] & tcp-syn != 0)' 2>/dev/null | head -20
        echo ""

        echo "--- All TCP control data (first 64 bytes of payload) ---"
        tcpdump -r "$PCAP_FILE" -n -X 'tcp port 2000 and tcp[tcpflags] & tcp-push != 0' 2>/dev/null | head -100
        echo ""

        echo "=== UDP Data Ports ==="
        echo ""
        echo "--- UDP port usage ---"
        tcpdump -r "$PCAP_FILE" -n 'udp' 2>/dev/null | awk '{print $3, $5}' | sort | uniq -c | sort -rn | head -20
        echo ""

        echo "--- Timing of first packets per connection ---"
        tcpdump -r "$PCAP_FILE" -n -tt 'tcp port 2000 and (tcp[tcpflags] & tcp-syn != 0)' 2>/dev/null | head -20
        echo ""

        echo "Full capture at: $PCAP_FILE"
        echo "Open in Wireshark: wireshark $PCAP_FILE"
        ;;

    *)
        echo "Usage: $0 <capture|analyze> [interface] [mikrotik_ip]"
        exit 1
        ;;
esac
