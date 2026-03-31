#!/usr/bin/env bash
# Test against a MikroTik device
# Usage: ./scripts/test-mikrotik.sh <mikrotik_ip> [username] [password]
set -euo pipefail

MIKROTIK_IP="${1:?Usage: $0 <mikrotik_ip> [username] [password]}"
USERNAME="${2:-}"
PASSWORD="${3:-}"
BTEST="./target/release/btest"

echo "=== Building release binary ==="
cargo build --release

AUTH_ARGS=""
if [ -n "$USERNAME" ]; then
    AUTH_ARGS="-a $USERNAME"
fi
if [ -n "$PASSWORD" ]; then
    AUTH_ARGS="$AUTH_ARGS -p $PASSWORD"
fi

echo ""
echo "=== Testing against MikroTik at $MIKROTIK_IP ==="
echo ""

TIMEOUT_CMD="timeout"
if ! command -v timeout &>/dev/null; then
    if command -v gtimeout &>/dev/null; then
        TIMEOUT_CMD="gtimeout"
    else
        TIMEOUT_CMD=""
    fi
fi

run_test() {
    local desc="$1"
    shift
    echo "--- $desc ---"
    if [ -n "$TIMEOUT_CMD" ]; then
        $TIMEOUT_CMD 10 $BTEST "$@" $AUTH_ARGS || echo "(test ended)"
    else
        $BTEST "$@" $AUTH_ARGS &
        local pid=$!
        sleep 10
        kill $pid 2>/dev/null || true
        wait $pid 2>/dev/null || true
        echo "(test ended)"
    fi
    echo ""
    sleep 1
}

echo "=== Mode 1: Our client -> MikroTik btest server ==="
echo "(Make sure btest server is enabled on your MikroTik: /tool/bandwidth-server set enabled=yes)"
echo ""

run_test "TCP Download from MikroTik" -c "$MIKROTIK_IP" -r
run_test "TCP Upload to MikroTik" -c "$MIKROTIK_IP" -t
run_test "TCP Bidirectional with MikroTik" -c "$MIKROTIK_IP" -t -r
run_test "UDP Download from MikroTik" -c "$MIKROTIK_IP" -r -u
run_test "UDP Upload to MikroTik" -c "$MIKROTIK_IP" -t -u

echo ""
echo "=== Mode 2: Our server <- MikroTik connects to us ==="
echo "To test this mode:"
echo "  1. Run:  $BTEST -s -v $AUTH_ARGS"
echo "  2. On MikroTik, run:"
echo "     /tool/bandwidth-test address=<this_server_ip> direction=both protocol=tcp"
echo ""
echo "=== All MikroTik tests completed ==="
