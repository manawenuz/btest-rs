#!/usr/bin/env bash
# Local loopback tests - run server and client against each other
set -euo pipefail

BTEST="cargo run --release --"
PORT=2000

echo "=== Building release binary ==="
cargo build --release

BTEST="./target/release/btest"

cleanup() {
    echo "Stopping server..."
    kill $SERVER_PID 2>/dev/null || true
    wait $SERVER_PID 2>/dev/null || true
}

echo ""
echo "=== Starting btest server on port $PORT ==="
$BTEST -s -P $PORT -v &
SERVER_PID=$!
trap cleanup EXIT
sleep 1

TIMEOUT_CMD="timeout"
if ! command -v timeout &>/dev/null; then
    if command -v gtimeout &>/dev/null; then
        TIMEOUT_CMD="gtimeout"
    else
        # Fallback: background + sleep + kill
        TIMEOUT_CMD=""
    fi
fi

run_test() {
    local desc="$1"
    shift
    echo ""
    echo "--- Test: $desc ---"
    if [ -n "$TIMEOUT_CMD" ]; then
        $TIMEOUT_CMD 5 $BTEST "$@" || true
    else
        $BTEST "$@" &
        local pid=$!
        sleep 5
        kill $pid 2>/dev/null || true
        wait $pid 2>/dev/null || true
    fi
    echo "--- Done: $desc ---"
    sleep 1
}

run_test "TCP Download (RX)" -c 127.0.0.1 -P $PORT -r
run_test "TCP Upload (TX)" -c 127.0.0.1 -P $PORT -t
run_test "TCP Bidirectional" -c 127.0.0.1 -P $PORT -t -r
run_test "TCP Download 100Mbps limited" -c 127.0.0.1 -P $PORT -r -b 100M
run_test "UDP Download" -c 127.0.0.1 -P $PORT -r -u
run_test "UDP Upload" -c 127.0.0.1 -P $PORT -t -u
run_test "UDP Bidirectional" -c 127.0.0.1 -P $PORT -t -r -u

echo ""
echo "=== All local tests completed ==="
