#!/usr/bin/env bash
# Install btest as a systemd service on Linux
# Usage: sudo ./install-service.sh [--auth-user USER --auth-pass PASS] [--port PORT]
set -euo pipefail

BTEST_BIN="/usr/local/bin/btest"
BTEST_USER="btest"
BTEST_PORT="2000"
AUTH_USER=""
AUTH_PASS=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --auth-user) AUTH_USER="$2"; shift 2 ;;
        --auth-pass) AUTH_PASS="$2"; shift 2 ;;
        --port)      BTEST_PORT="$2"; shift 2 ;;
        --help|-h)
            echo "Usage: sudo $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --auth-user USER   Authentication username"
            echo "  --auth-pass PASS   Authentication password"
            echo "  --port PORT        Listen port (default: 2000)"
            echo ""
            echo "Examples:"
            echo "  sudo $0"
            echo "  sudo $0 --auth-user admin --auth-pass secret"
            echo "  sudo $0 --port 2000 --auth-user admin --auth-pass mypass"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [[ $EUID -ne 0 ]]; then
    echo "Error: This script must be run as root (use sudo)"
    exit 1
fi

# Check binary exists
if [[ ! -f "$BTEST_BIN" ]]; then
    # Try to find it in current directory or dist/
    if [[ -f "./btest" ]]; then
        cp ./btest "$BTEST_BIN"
    elif [[ -f "./dist/btest" ]]; then
        cp ./dist/btest "$BTEST_BIN"
    else
        echo "Error: btest binary not found. Copy it to $BTEST_BIN first."
        echo "  scp dist/btest root@server:/usr/local/bin/btest"
        exit 1
    fi
fi

chmod +x "$BTEST_BIN"

# Create service user
if ! id -u "$BTEST_USER" &>/dev/null; then
    useradd --system --no-create-home --shell /usr/sbin/nologin "$BTEST_USER"
    echo "Created system user: $BTEST_USER"
fi

# Build ExecStart command
EXEC_START="$BTEST_BIN -s -P $BTEST_PORT"
if [[ -n "$AUTH_USER" ]]; then
    EXEC_START="$EXEC_START -a $AUTH_USER"
fi
if [[ -n "$AUTH_PASS" ]]; then
    EXEC_START="$EXEC_START -p $AUTH_PASS"
fi

# Create systemd unit
cat > /etc/systemd/system/btest.service << UNIT
[Unit]
Description=MikroTik Bandwidth Test Server (btest)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$BTEST_USER
ExecStart=$EXEC_START
Restart=always
RestartSec=5

# Security hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectControlGroups=yes

# Allow binding to port 2000 (< 1024 needs capability)
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE

# Resource limits
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
UNIT

echo "Created /etc/systemd/system/btest.service"

# Reload and enable
systemctl daemon-reload
systemctl enable btest.service
systemctl restart btest.service

echo ""
echo "=== btest service installed and started ==="
echo ""
systemctl status btest.service --no-pager
echo ""
echo "Useful commands:"
echo "  systemctl status btest      # Check status"
echo "  systemctl stop btest        # Stop"
echo "  systemctl restart btest     # Restart"
echo "  journalctl -u btest -f      # Follow logs"
echo "  systemctl disable btest     # Disable autostart"
