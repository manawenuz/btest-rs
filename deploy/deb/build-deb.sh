#!/usr/bin/env bash
# build-deb.sh -- Build a Debian/Ubuntu .deb package for btest-rs
#
# Usage:
#   ./deploy/deb/build-deb.sh              # uses dist/btest or target/release/btest
#   BTEST_BIN=path/to/btest ./deploy/deb/build-deb.sh
#
# Requirements: dpkg-deb, gzip (standard on Debian/Ubuntu build hosts)
set -euo pipefail

###############################################################################
# Package metadata
###############################################################################
PKG_NAME="btest-rs"
PKG_VERSION="0.6.0"
PKG_ARCH="amd64"
PKG_MAINTAINER="Siavash Sameni <manwe@manko.yoga>"
PKG_DESCRIPTION="MikroTik Bandwidth Test (btest) server and client with EC-SRP5 auth"
PKG_HOMEPAGE="https://github.com/manawenuz/btest-rs"
PKG_LICENSE="MIT AND Apache-2.0"
PKG_SECTION="net"
PKG_PRIORITY="optional"

###############################################################################
# Paths
###############################################################################
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Locate the pre-built binary
if [[ -n "${BTEST_BIN:-}" ]]; then
    : # caller provided an explicit path
elif [[ -f "$REPO_ROOT/dist/btest" ]]; then
    BTEST_BIN="$REPO_ROOT/dist/btest"
elif [[ -f "$REPO_ROOT/target/release/btest" ]]; then
    BTEST_BIN="$REPO_ROOT/target/release/btest"
else
    echo "Error: cannot find btest binary."
    echo "  Build first (cargo build --release) or set BTEST_BIN=path/to/btest"
    exit 1
fi

# Verify the binary exists and is executable
if [[ ! -f "$BTEST_BIN" ]]; then
    echo "Error: $BTEST_BIN does not exist."
    exit 1
fi

echo "==> Using binary: $BTEST_BIN"

###############################################################################
# Prepare staging tree
###############################################################################
DEB_FILE="${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "==> Staging in $STAGE"

# Binary
install -Dm755 "$BTEST_BIN"                         "$STAGE/usr/bin/btest"

# Man page
if [[ -f "$REPO_ROOT/docs/man/btest.1" ]]; then
    install -Dm644 "$REPO_ROOT/docs/man/btest.1"    "$STAGE/usr/share/man/man1/btest.1"
    gzip -9n "$STAGE/usr/share/man/man1/btest.1"
else
    echo "Warning: docs/man/btest.1 not found -- skipping man page"
fi

# systemd service unit
install -d "$STAGE/usr/lib/systemd/system"
cat > "$STAGE/usr/lib/systemd/system/btest.service" <<'UNIT'
[Unit]
Description=MikroTik Bandwidth Test Server (btest-rs)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/btest -s
Restart=always
RestartSec=5
DynamicUser=yes
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectControlGroups=yes
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
UNIT

# Documentation
install -Dm644 "$REPO_ROOT/README.md"               "$STAGE/usr/share/doc/$PKG_NAME/README.md"

# License
install -Dm644 "$REPO_ROOT/LICENSE"                  "$STAGE/usr/share/licenses/$PKG_NAME/LICENSE"

# Debian copyright file (policy-compliant copy in /usr/share/doc)
install -d "$STAGE/usr/share/doc/$PKG_NAME"
cat > "$STAGE/usr/share/doc/$PKG_NAME/copyright" <<COPY
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: $PKG_NAME
Upstream-Contact: $PKG_MAINTAINER
Source: $PKG_HOMEPAGE

Files: *
Copyright: 2024-2026 Siavash Sameni
License: MIT AND Apache-2.0
COPY

###############################################################################
# Calculate installed size (in KiB, as Debian policy requires)
###############################################################################
INSTALLED_SIZE=$(du -sk "$STAGE" | cut -f1)

###############################################################################
# DEBIAN/control
###############################################################################
install -d "$STAGE/DEBIAN"
cat > "$STAGE/DEBIAN/control" <<CTRL
Package: $PKG_NAME
Version: $PKG_VERSION
Architecture: $PKG_ARCH
Maintainer: $PKG_MAINTAINER
Installed-Size: $INSTALLED_SIZE
Section: $PKG_SECTION
Priority: $PKG_PRIORITY
Homepage: $PKG_HOMEPAGE
Description: $PKG_DESCRIPTION
 A high-performance Rust implementation of the MikroTik Bandwidth Test
 protocol, supporting both server and client modes with EC-SRP5
 authentication. Supports TCP/UDP throughput testing and is fully
 compatible with RouterOS btest clients.
CTRL

###############################################################################
# DEBIAN/conffiles  (mark the systemd unit as a conffile)
###############################################################################
cat > "$STAGE/DEBIAN/conffiles" <<'CF'
/usr/lib/systemd/system/btest.service
CF

###############################################################################
# Maintainer scripts
###############################################################################

# postinst -- reload systemd after install
cat > "$STAGE/DEBIAN/postinst" <<'POST'
#!/bin/sh
set -e
if [ "$1" = "configure" ]; then
    if command -v systemctl >/dev/null 2>&1; then
        systemctl daemon-reload || true
        echo ""
        echo "btest-rs installed.  To start the server:"
        echo "  sudo systemctl enable --now btest.service"
        echo ""
    fi
fi
POST
chmod 755 "$STAGE/DEBIAN/postinst"

# prerm -- stop service before removal
cat > "$STAGE/DEBIAN/prerm" <<'PRERM'
#!/bin/sh
set -e
if [ "$1" = "remove" ] || [ "$1" = "deconfigure" ]; then
    if command -v systemctl >/dev/null 2>&1; then
        systemctl stop btest.service 2>/dev/null || true
        systemctl disable btest.service 2>/dev/null || true
    fi
fi
PRERM
chmod 755 "$STAGE/DEBIAN/prerm"

# postrm -- clean up after removal
cat > "$STAGE/DEBIAN/postrm" <<'POSTRM'
#!/bin/sh
set -e
if [ "$1" = "purge" ] || [ "$1" = "remove" ]; then
    if command -v systemctl >/dev/null 2>&1; then
        systemctl daemon-reload || true
    fi
fi
POSTRM
chmod 755 "$STAGE/DEBIAN/postrm"

###############################################################################
# Build .deb
###############################################################################
OUTPUT_DIR="${OUTPUT_DIR:-$REPO_ROOT/dist}"
mkdir -p "$OUTPUT_DIR"

echo "==> Building $DEB_FILE ..."
dpkg-deb --root-owner-group --build "$STAGE" "$OUTPUT_DIR/$DEB_FILE"

echo "==> Package ready: $OUTPUT_DIR/$DEB_FILE"
echo ""
dpkg-deb --info "$OUTPUT_DIR/$DEB_FILE"
echo ""
dpkg-deb --contents "$OUTPUT_DIR/$DEB_FILE"
