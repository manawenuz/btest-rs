#!/usr/bin/env bash
# Build and push multi-arch Docker images to Gitea registry.
#
# Prerequisites:
#   - dist/btest-linux-x86_64.tar.gz (from CI release or scripts/build-linux.sh)
#   - Native macOS binary (built automatically)
#
# Usage:
#   ./scripts/push-docker.sh v0.1.0
set -euo pipefail

cd "$(dirname "$0")/.."

# Load .env if present
if [[ -f .env ]]; then
    set -a
    source .env
    set +a
fi

TAG="${1:?Usage: $0 <tag>  (e.g. v0.1.0)}"
REGISTRY_HOST="${GITEA_URL:-https://git.manko.yoga}"
REGISTRY_HOST="${REGISTRY_HOST#https://}"
REGISTRY_HOST="${REGISTRY_HOST#http://}"
IMAGE="${REGISTRY_HOST}/manawenuz/btest-rs"

# Login
if [[ -n "${GITEA_TOKEN:-}" ]]; then
    DOCKER_USER="${GITEA_USER:?Set GITEA_USER in .env}"
    echo "${GITEA_TOKEN}" | docker login "${REGISTRY_HOST}" -u "${DOCKER_USER}" --password-stdin
fi

mkdir -p dist/docker-amd64 dist/docker-arm64

# --- x86_64 binary ---
if [[ ! -f dist/docker-amd64/btest ]]; then
    if [[ -f dist/btest-linux-x86_64.tar.gz ]]; then
        echo "Extracting x86_64 binary from tarball..."
        tar xzf dist/btest-linux-x86_64.tar.gz -C dist/docker-amd64/
    else
        echo "No x86_64 binary found. Downloading from release ${TAG}..."
        GITEA_URL_FULL="https://${REGISTRY_HOST}"
        RELEASE_URL=$(curl -sf \
            -H "Authorization: token ${GITEA_TOKEN}" \
            "${GITEA_URL_FULL}/api/v1/repos/manawenuz/btest-rs/releases/tags/${TAG}" \
            | jq -r '.assets[] | select(.name=="btest-linux-x86_64.tar.gz") | .browser_download_url')
        if [[ -n "$RELEASE_URL" ]]; then
            curl -sL "$RELEASE_URL" | tar xz -C dist/docker-amd64/
        else
            echo "Error: Cannot find x86_64 binary. Run CI first or scripts/build-linux.sh"
            exit 1
        fi
    fi
fi

# --- arm64 binary (native build on Apple Silicon) ---
if [[ ! -f dist/docker-arm64/btest ]]; then
    echo "Building native arm64 binary..."
    cargo build --release
    cp target/release/btest dist/docker-arm64/btest
fi

echo ""
echo "=== Building amd64 image ==="
docker build --platform linux/amd64 -f Dockerfile.static \
    --build-arg BINARY=dist/docker-amd64/btest \
    -t "${IMAGE}:${TAG}-amd64" .

echo ""
echo "=== Building arm64 image ==="
docker build --platform linux/arm64 -f Dockerfile.static \
    --build-arg BINARY=dist/docker-arm64/btest \
    -t "${IMAGE}:${TAG}-arm64" .

echo ""
echo "=== Pushing ==="
docker push "${IMAGE}:${TAG}-amd64"
docker push "${IMAGE}:${TAG}-arm64"

# Create and push multi-arch manifest
echo ""
echo "=== Creating multi-arch manifest ==="
docker manifest create "${IMAGE}:${TAG}" \
    "${IMAGE}:${TAG}-amd64" \
    "${IMAGE}:${TAG}-arm64" 2>/dev/null || \
docker manifest create --amend "${IMAGE}:${TAG}" \
    "${IMAGE}:${TAG}-amd64" \
    "${IMAGE}:${TAG}-arm64"

docker manifest push "${IMAGE}:${TAG}"

# Tag as latest
docker manifest create "${IMAGE}:latest" \
    "${IMAGE}:${TAG}-amd64" \
    "${IMAGE}:${TAG}-arm64" 2>/dev/null || \
docker manifest create --amend "${IMAGE}:latest" \
    "${IMAGE}:${TAG}-amd64" \
    "${IMAGE}:${TAG}-arm64"

docker manifest push "${IMAGE}:latest"

echo ""
echo "Done! Multi-arch images pushed:"
echo "  ${IMAGE}:${TAG}      (amd64 + arm64)"
echo "  ${IMAGE}:latest      (amd64 + arm64)"
echo ""
echo "Run with:"
echo "  docker run --rm -p 2000:2000 -p 2001-2100:2001-2100/udp ${IMAGE}:${TAG} -s -v"
