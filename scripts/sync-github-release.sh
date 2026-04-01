#!/usr/bin/env bash
# Sync a release from Gitea to GitHub.
# Downloads all binaries from Gitea release, creates GitHub release, uploads them.
#
# Prerequisites:
#   gh auth login   (GitHub CLI authenticated)
#
# Usage:
#   ./scripts/sync-github-release.sh v0.6.0
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -f .env ]]; then
    set -a; source .env; set +a
fi

TAG="${1:?Usage: $0 <tag> (e.g. v0.6.0)}"
GITEA_URL="https://git.manko.yoga"
GITEA_REPO="manawenuz/btest-rs"
GITHUB_REPO="manawenuz/btest-rs"

echo "=== Downloading assets from Gitea release ${TAG} ==="
mkdir -p /tmp/btest-release-${TAG}
cd /tmp/btest-release-${TAG}
rm -f *.tar.gz *.zip *.txt

# Get asset list from Gitea API
ASSETS=$(curl -sf "${GITEA_URL}/api/v1/repos/${GITEA_REPO}/releases/tags/${TAG}" | \
    python3 -c "import sys,json; [print(a['browser_download_url']) for a in json.load(sys.stdin).get('assets',[])]")

if [ -z "$ASSETS" ]; then
    echo "No assets found for ${TAG} on Gitea. Check if the release exists."
    exit 1
fi

for url in $ASSETS; do
    FILENAME=$(basename "$url")
    echo "  Downloading: $FILENAME"
    curl -sLO "$url"
done

# Merge all separate .sha256 files into checksums-sha256.txt
# and remove the individual .sha256 files
echo ""
echo "=== Merging checksums ==="
for sha_file in *.sha256; do
    [ -f "$sha_file" ] || continue
    echo "  Merging: $sha_file"
    cat "$sha_file" >> checksums-sha256.txt
    rm "$sha_file"
done

# Add checksums for any files not yet in checksums-sha256.txt
for f in *.tar.gz *.zip; do
    [ -f "$f" ] || continue
    if ! grep -q "$f" checksums-sha256.txt 2>/dev/null; then
        echo "  Adding checksum for: $f"
        shasum -a 256 "$f" >> checksums-sha256.txt
    fi
done

# Sort and deduplicate
sort -u -k2 checksums-sha256.txt > checksums-sha256.tmp && mv checksums-sha256.tmp checksums-sha256.txt

echo ""
echo "Checksums:"
cat checksums-sha256.txt

echo ""
echo "Files to upload:"
ls -lh *.tar.gz *.zip checksums-sha256.txt 2>/dev/null

echo ""
echo "=== Creating GitHub release ${TAG} ==="
gh release create "${TAG}" \
    --repo "${GITHUB_REPO}" \
    --title "btest-rs ${TAG}" \
    --notes "## Downloads

| Platform | Architecture | File |
|----------|-------------|------|
| Linux | x86_64 | btest-linux-x86_64.tar.gz |
| Linux | aarch64 (RPi 64-bit) | btest-linux-aarch64.tar.gz |
| Linux | armv7 (RPi 32-bit) | btest-linux-armv7.tar.gz |
| Windows | x86_64 | btest-windows-x86_64.zip |
| macOS | aarch64 (Apple Silicon) | btest-darwin-aarch64.tar.gz |
| Docker | x86_64 | \`docker pull ghcr.io/manawenuz/btest-rs:${TAG}\` |

### Quick Install (Linux)

\`\`\`bash
curl -LO https://github.com/${GITHUB_REPO}/releases/download/${TAG}/btest-linux-x86_64.tar.gz
tar xzf btest-linux-x86_64.tar.gz
sudo mv btest /usr/local/bin/
\`\`\`

### Raspberry Pi

\`\`\`bash
# 64-bit
curl -LO https://github.com/${GITHUB_REPO}/releases/download/${TAG}/btest-linux-aarch64.tar.gz
tar xzf btest-linux-aarch64.tar.gz
sudo mv btest /usr/local/bin/

# 32-bit
curl -LO https://github.com/${GITHUB_REPO}/releases/download/${TAG}/btest-linux-armv7.tar.gz
tar xzf btest-linux-armv7.tar.gz
sudo mv btest /usr/local/bin/
\`\`\`
" \
    ./*.tar.gz ./*.zip ./*.txt 2>/dev/null || true

echo ""
echo "=== Done! ==="
echo "https://github.com/${GITHUB_REPO}/releases/tag/${TAG}"

# Cleanup
cd -
rm -rf /tmp/btest-release-${TAG}
