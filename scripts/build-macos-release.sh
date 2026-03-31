#!/usr/bin/env bash
# Build macOS binaries and optionally upload to a Gitea release.
# Run this on a macOS host (Intel or Apple Silicon).
#
# Usage:
#   ./scripts/build-macos-release.sh                    # build only
#   ./scripts/build-macos-release.sh --upload v0.1.0    # build + upload to release tag
set -euo pipefail

cd "$(dirname "$0")/.."

GITEA_URL="https://git.manko.yoga"
REPO="manawenuz/btest-rs"
UPLOAD=""
TAG=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --upload) UPLOAD=1; TAG="$2"; shift 2 ;;
        *) echo "Usage: $0 [--upload TAG]"; exit 1 ;;
    esac
done

echo "=== Building macOS release binary ==="
cargo build --release

ARCH=$(uname -m)
case "$ARCH" in
    x86_64) PLATFORM="darwin-x86_64" ;;
    arm64)  PLATFORM="darwin-aarch64" ;;
    *)      PLATFORM="darwin-${ARCH}" ;;
esac

mkdir -p dist
TARBALL="dist/btest-${PLATFORM}.tar.gz"
cd target/release
tar czf "../../${TARBALL}" btest
cd ../..
shasum -a 256 "${TARBALL}" > "${TARBALL}.sha256"

echo "Built: ${TARBALL}"
ls -lh "${TARBALL}"
cat "${TARBALL}.sha256"

if [[ -n "$UPLOAD" ]]; then
    if [[ -z "${GITEA_TOKEN:-}" ]]; then
        echo ""
        echo "Set GITEA_TOKEN to upload. Example:"
        echo "  export GITEA_TOKEN=your_token_here"
        echo "  $0 --upload ${TAG}"
        exit 1
    fi

    echo ""
    echo "=== Uploading to release ${TAG} ==="

    # Find release ID by tag
    RELEASE_ID=$(curl -s \
        -H "Authorization: token ${GITEA_TOKEN}" \
        "${GITEA_URL}/api/v1/repos/${REPO}/releases/tags/${TAG}" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")

    echo "Release ID: ${RELEASE_ID}"

    for file in "${TARBALL}" "${TARBALL}.sha256"; do
        FILENAME=$(basename "$file")
        echo "Uploading: ${FILENAME}"
        curl -s -X POST \
            -H "Authorization: token ${GITEA_TOKEN}" \
            -F "attachment=@${file}" \
            "${GITEA_URL}/api/v1/repos/${REPO}/releases/${RELEASE_ID}/assets?name=${FILENAME}"
        echo ""
    done

    echo "Done! macOS binary uploaded to ${TAG}"
fi
