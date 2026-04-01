#!/usr/bin/env bash
# Test the AUR package on a remote x86_64 Linux server using Docker.
#
# Usage:
#   ./scripts/test-aur-remote.sh [user@host]
#
# If no host is given, runs locally (must be x86_64 Linux with Docker).
# The script SSHes to the remote, runs an Arch container, installs from AUR,
# tests loopback, and cleans up.
set -euo pipefail

REMOTE="${1:-}"

TEST_SCRIPT='
docker run --rm archlinux:latest bash -c "
set -euo pipefail

echo \"[1/5] Installing build tools...\"
pacman -Syu --noconfirm base-devel rustup git sudo >/dev/null 2>&1

echo \"[2/5] Setting up Rust...\"
rustup default stable >/dev/null 2>&1

echo \"[3/5] Creating build user...\"
useradd -m builder
echo \"builder ALL=(ALL) NOPASSWD: ALL\" >> /etc/sudoers

echo \"[4/5] Building btest-rs from AUR...\"
su builder -c \"
    cd /tmp
    git clone https://aur.archlinux.org/btest-rs.git 2>/dev/null
    cd btest-rs
    makepkg -si --noconfirm 2>&1 | tail -5
\"

echo \"\"
echo \"[5/5] Testing...\"
echo \"--- Version ---\"
btest --version

echo \"--- Installed files ---\"
pacman -Ql btest-rs

echo \"--- Loopback test (TCP, 3s) ---\"
btest -s -P 19876 &
sleep 2
btest -c 127.0.0.1 -P 19876 -r -d 3
kill %1 2>/dev/null || true
wait 2>/dev/null || true

echo \"\"
echo \"--- Loopback test (UDP, 3s) ---\"
btest -s -P 19877 &
sleep 2
btest -c 127.0.0.1 -P 19877 -r -u -d 3
kill %1 2>/dev/null || true
wait 2>/dev/null || true

echo \"\"
echo \"=== ALL TESTS PASSED ===\"
"
'

if [ -n "$REMOTE" ]; then
    echo "=== Testing AUR package on $REMOTE ==="
    ssh "$REMOTE" "$TEST_SCRIPT"
else
    echo "=== Testing AUR package locally ==="
    eval "$TEST_SCRIPT"
fi
