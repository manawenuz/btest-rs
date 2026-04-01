#!/usr/bin/env bash
# Build and push Docker image to both Gitea and GitHub Container Registry.
#
# Prerequisites:
#   docker login git.manko.yoga        (Gitea — your username + token)
#   docker login ghcr.io               (GitHub — your username + PAT with packages:write)
#
# Usage:
#   ./scripts/push-docker-all.sh v0.6.0
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -f .env ]]; then
    set -a; source .env; set +a
fi

TAG="${1:?Usage: $0 <tag> (e.g. v0.6.0)}"

GITEA_IMAGE="git.manko.yoga/manawenuz/btest-rs"
GHCR_IMAGE="ghcr.io/manawenuz/btest-rs"

echo "=== Building Docker image ==="
docker build \
    -t "${GITEA_IMAGE}:${TAG}" \
    -t "${GITEA_IMAGE}:latest" \
    -t "${GHCR_IMAGE}:${TAG}" \
    -t "${GHCR_IMAGE}:latest" \
    .

echo ""
echo "=== Pushing to Gitea ==="
docker push "${GITEA_IMAGE}:${TAG}"
docker push "${GITEA_IMAGE}:latest"

echo ""
echo "=== Pushing to GitHub Container Registry ==="
docker push "${GHCR_IMAGE}:${TAG}"
docker push "${GHCR_IMAGE}:latest"

echo ""
echo "Done! Images pushed:"
echo "  ${GITEA_IMAGE}:${TAG}"
echo "  ${GITEA_IMAGE}:latest"
echo "  ${GHCR_IMAGE}:${TAG}"
echo "  ${GHCR_IMAGE}:latest"
echo ""
echo "Pull with:"
echo "  docker pull ${GHCR_IMAGE}:${TAG}"
echo "  docker run --rm -p 2000:2000 -p 2001-2100:2001-2100/udp ${GHCR_IMAGE}:${TAG} -s -v"
