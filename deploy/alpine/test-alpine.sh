#!/bin/sh
# Test Alpine Linux packaging for btest-rs
# Runs inside an Alpine Docker container to build and verify the APK.
#
# Usage (from repository root):
#   docker run --rm -v "$PWD":/src alpine:latest /src/deploy/alpine/test-alpine.sh
#
set -eu

ALPINE_DIR="/src/deploy/alpine"

echo "=== Alpine APK packaging test ==="
echo "Alpine version: $(cat /etc/alpine-release)"

# ── Install build dependencies ──────────────────────────────────────
echo "--- Installing build dependencies ---"
apk update
apk add --no-cache \
    alpine-sdk \
    rust \
    cargo \
    sudo

# ── Create a non-root build user (abuild refuses to run as root) ──
echo "--- Setting up build user ---"
adduser -D builder
addgroup builder abuild
echo "builder ALL=(ALL) NOPASSWD: ALL" >> /etc/sudoers

# ── Prepare build tree ──────────────────────────────────────────────
echo "--- Preparing build tree ---"
BUILD_DIR="/home/builder/btest-rs"
mkdir -p "$BUILD_DIR"
cp "$ALPINE_DIR/APKBUILD" "$BUILD_DIR/"
cp "$ALPINE_DIR/btest.initd" "$BUILD_DIR/"

# Generate signing key (required by abuild)
su builder -c "abuild-keygen -a -n -q"
sudo cp /home/builder/.abuild/*.rsa.pub /etc/apk/keys/

# ── Build the package ──────────────────────────────────────────────
echo "--- Building APK ---"
cd "$BUILD_DIR"
chown -R builder:builder "$BUILD_DIR"
su builder -c "abuild -r"

echo "--- Build succeeded ---"

# ── Locate and install the package ──────────────────────────────────
echo "--- Installing built APK ---"
APK_FILE=$(find /home/builder/packages -name "btest-rs-*.apk" -not -name "*doc*" | head -1)
if [ -z "$APK_FILE" ]; then
    echo "FAIL: APK file not found"
    exit 1
fi
echo "Found APK: $APK_FILE"
apk add --allow-untrusted "$APK_FILE"

# ── Verify installation ────────────────────────────────────────────
echo "--- Verifying installation ---"
FAIL=0

# Binary exists and is executable
if command -v btest >/dev/null 2>&1; then
    echo "PASS: btest binary installed"
else
    echo "FAIL: btest binary not found in PATH"
    FAIL=1
fi

# Binary runs (show version / help)
if btest --help >/dev/null 2>&1; then
    echo "PASS: btest --help exits successfully"
else
    echo "FAIL: btest --help failed"
    FAIL=1
fi

# Man page installed
if [ -f /usr/share/man/man1/btest.1 ]; then
    echo "PASS: man page installed"
else
    echo "FAIL: man page not found"
    FAIL=1
fi

# License installed
if [ -f /usr/share/licenses/btest-rs/LICENSE ]; then
    echo "PASS: LICENSE installed"
else
    echo "FAIL: LICENSE not found"
    FAIL=1
fi

# OpenRC init script installed
if [ -f /etc/init.d/btest ]; then
    echo "PASS: OpenRC init script installed"
else
    echo "FAIL: OpenRC init script not found"
    FAIL=1
fi

# Init script is executable
if [ -x /etc/init.d/btest ]; then
    echo "PASS: init script is executable"
else
    echo "FAIL: init script is not executable"
    FAIL=1
fi

# ── Summary ─────────────────────────────────────────────────────────
echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "=== All Alpine packaging tests PASSED ==="
else
    echo "=== Some Alpine packaging tests FAILED ==="
    exit 1
fi
