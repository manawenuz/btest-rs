#!/usr/bin/env bash
# Build an OpenWrt .ipk package from a pre-built static binary.
# No OpenWrt SDK needed — just packages the binary with metadata.
#
# Usage:
#   ./deploy/openwrt/build-ipk.sh <arch> [binary-path]
#
# Examples:
#   ./deploy/openwrt/build-ipk.sh x86_64 dist/btest           # from cross-compiled binary
#   ./deploy/openwrt/build-ipk.sh aarch64 dist/btest           # for RPi/ARM64 routers
#   ./deploy/openwrt/build-ipk.sh mipsel target/release/btest  # for MIPS little-endian
#
# Supported architectures: x86_64, aarch64, arm_cortex-a7, mipsel_24kc, mips_24kc
set -euo pipefail

cd "$(dirname "$0")/../.."

ARCH="${1:?Usage: $0 <arch> [binary-path]}"
BINARY="${2:-dist/btest}"
VERSION="0.6.0"
PKG_NAME="btest-rs"
OUTPUT_DIR="dist"

if [ ! -f "$BINARY" ]; then
    echo "Error: binary not found at $BINARY"
    echo "Build it first: cargo build --release --target <target>"
    exit 1
fi

mkdir -p "$OUTPUT_DIR"
WORKDIR=$(mktemp -d)
trap "rm -rf $WORKDIR" EXIT

echo "=== Building ${PKG_NAME}_${VERSION}_${ARCH}.ipk ==="

# Create package structure
mkdir -p "$WORKDIR/data/usr/bin"
mkdir -p "$WORKDIR/data/etc/init.d"
mkdir -p "$WORKDIR/data/etc/config"
mkdir -p "$WORKDIR/control"

# Install files
cp "$BINARY" "$WORKDIR/data/usr/bin/btest"
chmod 755 "$WORKDIR/data/usr/bin/btest"
cp deploy/openwrt/files/btest.init "$WORKDIR/data/etc/init.d/btest"
chmod 755 "$WORKDIR/data/etc/init.d/btest"
cp deploy/openwrt/files/btest.config "$WORKDIR/data/etc/config/btest"

# Calculate installed size
INSTALLED_SIZE=$(du -sk "$WORKDIR/data" | awk '{print $1}')

# Control file
cat > "$WORKDIR/control/control" << EOF
Package: ${PKG_NAME}
Version: ${VERSION}-1
Depends: libc
Source: https://github.com/manawenuz/btest-rs
License: MIT AND Apache-2.0
Section: net
SourceName: ${PKG_NAME}
Maintainer: Siavash Sameni <manwe@manko.yoga>
Architecture: ${ARCH}
Installed-Size: ${INSTALLED_SIZE}
Description: MikroTik Bandwidth Test server and client
 A Rust reimplementation of the MikroTik btest protocol.
 Supports TCP/UDP, EC-SRP5 and MD5 auth, IPv4/IPv6.
EOF

# Post-install script
cat > "$WORKDIR/control/postinst" << 'EOF'
#!/bin/sh
[ "${IPKG_NO_SCRIPT}" = "1" ] && exit 0
/etc/init.d/btest enable 2>/dev/null || true
exit 0
EOF
chmod 755 "$WORKDIR/control/postinst"

# Pre-remove script
cat > "$WORKDIR/control/prerm" << 'EOF'
#!/bin/sh
/etc/init.d/btest stop 2>/dev/null || true
/etc/init.d/btest disable 2>/dev/null || true
exit 0
EOF
chmod 755 "$WORKDIR/control/prerm"

# Conffiles
cat > "$WORKDIR/control/conffiles" << EOF
/etc/config/btest
EOF

# Build the .ipk (it's just a tar.gz of tar.gz's)
cd "$WORKDIR"

# Create data.tar.gz
(cd data && tar czf ../data.tar.gz .)

# Create control.tar.gz
(cd control && tar czf ../control.tar.gz .)

# Create debian-binary
echo "2.0" > debian-binary

# Package it all
tar czf "${PKG_NAME}_${VERSION}-1_${ARCH}.ipk" debian-binary control.tar.gz data.tar.gz

cd -
cp "$WORKDIR/${PKG_NAME}_${VERSION}-1_${ARCH}.ipk" "$OUTPUT_DIR/"

echo ""
echo "Package: $OUTPUT_DIR/${PKG_NAME}_${VERSION}-1_${ARCH}.ipk"
ls -lh "$OUTPUT_DIR/${PKG_NAME}_${VERSION}-1_${ARCH}.ipk"
echo ""
echo "Install on OpenWrt:"
echo "  scp $OUTPUT_DIR/${PKG_NAME}_${VERSION}-1_${ARCH}.ipk root@router:/tmp/"
echo "  ssh root@router 'opkg install /tmp/${PKG_NAME}_${VERSION}-1_${ARCH}.ipk'"
echo "  ssh root@router '/etc/init.d/btest enable && /etc/init.d/btest start'"
