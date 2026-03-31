#!/usr/bin/env bash
# Build and push Docker image to Gitea registry.
# Run on a machine with Docker (your Mac).
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
REGISTRY="${GITEA_URL:-https://git.manko.yoga}"
REGISTRY_HOST="${REGISTRY#https://}"
REGISTRY_HOST="${REGISTRY_HOST#http://}"
IMAGE="${REGISTRY_HOST}/manawenuz/btest-rs"

echo "=== Building Docker image for ${IMAGE}:${TAG} ==="

# Build the image
docker build -t "${IMAGE}:${TAG}" -t "${IMAGE}:latest" .

echo ""
echo "=== Pushing to ${REGISTRY_HOST} ==="

# Login if needed (uses GITEA_USER + GITEA_TOKEN from .env)
if [[ -n "${GITEA_TOKEN:-}" ]]; then
    DOCKER_USER="${GITEA_USER:?Set GITEA_USER in .env (your Gitea username)}"
    echo "${GITEA_TOKEN}" | docker login "${REGISTRY_HOST}" -u "${DOCKER_USER}" --password-stdin
fi

docker push "${IMAGE}:${TAG}"
docker push "${IMAGE}:latest"

echo ""
echo "Done! Images pushed:"
echo "  ${IMAGE}:${TAG}"
echo "  ${IMAGE}:latest"
echo ""
echo "Run with:"
echo "  docker run --rm -p 2000:2000 -p 2001-2100:2001-2100/udp ${IMAGE}:${TAG} -s -v"
