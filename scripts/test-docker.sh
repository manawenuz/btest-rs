#!/usr/bin/env bash
# Test using Docker
set -euo pipefail

echo "=== Building Docker image ==="
docker build -t btest .

echo ""
echo "=== Running btest server in Docker ==="
docker run -d --name btest-test -p 2000:2000/tcp -p 2001-2100:2001-2100/udp -p 2257-2356:2257-2356/udp btest -s -v
sleep 2

cleanup() {
    echo "Stopping Docker container..."
    docker stop btest-test 2>/dev/null || true
    docker rm btest-test 2>/dev/null || true
}
trap cleanup EXIT

BTEST="./target/release/btest"
cargo build --release

run_timed() {
    local desc="$1"; shift
    echo ""
    echo "--- $desc ---"
    $BTEST "$@" &
    local pid=$!
    sleep 5
    kill $pid 2>/dev/null || true
    wait $pid 2>/dev/null || true
}

run_timed "TCP Download from Docker server" -c 127.0.0.1 -r
run_timed "TCP Upload to Docker server" -c 127.0.0.1 -t
run_timed "TCP Bidirectional" -c 127.0.0.1 -t -r

echo ""
echo "=== Docker server logs ==="
docker logs btest-test

echo ""
echo "=== Docker tests completed ==="
