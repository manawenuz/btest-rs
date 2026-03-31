#!/usr/bin/env bash
# Build a static x86_64 Linux binary using Docker (works from macOS)
set -euo pipefail

cd "$(dirname "$0")/.."

echo "=== Building x86_64 Linux binary via Docker ==="
DOCKER_BUILDKIT=1 docker build \
    -f Dockerfile.cross \
    --output type=local,dest=./dist \
    .

ls -lh dist/btest
file dist/btest
echo ""
echo "Binary ready at: dist/btest"
echo "Copy to your server: scp dist/btest user@server:/usr/local/bin/btest"
