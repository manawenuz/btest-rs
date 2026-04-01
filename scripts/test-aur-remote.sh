#!/usr/bin/env bash
# Test the AUR package on a remote x86_64 Linux server using Docker.
#
# Usage:
#   ./scripts/test-aur-remote.sh [user@host]
#
# Spins up an Arch container, installs btest-rs via yay (like a real user),
# runs loopback tests, cleans up.
set -euo pipefail

REMOTE="${1:-}"

TEST_SCRIPT='
docker run --rm archlinux:latest bash -c "
set -euo pipefail

echo \"[1/4] Installing yay...\"
pacman -Syu --noconfirm base-devel git sudo >/dev/null 2>&1
useradd -m builder
echo \"builder ALL=(ALL) NOPASSWD: ALL\" >> /etc/sudoers
su builder -c \"
    cd /tmp
    git clone https://aur.archlinux.org/yay-bin.git 2>/dev/null
    cd yay-bin
    makepkg -si --noconfirm 2>&1 | tail -3
\"

echo \"[2/4] Installing btest-rs from AUR via yay...\"
su builder -c \"yay -S btest-rs --noconfirm 2>&1 | tail -10\"

echo \"\"
echo \"[3/4] Verify installation...\"
btest --version
which btest
man -w btest.1 2>/dev/null && echo \"Man page: installed\" || echo \"Man page: not found\"
systemctl cat btest.service 2>/dev/null | head -3 && echo \"Systemd unit: installed\" || echo \"Systemd unit: not found\"

echo \"\"
echo \"[4/4] Loopback tests...\"

echo \"--- TCP (3s) ---\"
btest -s -P 19876 &
sleep 2
btest -c 127.0.0.1 -P 19876 -r -d 3
kill %1 2>/dev/null; wait 2>/dev/null || true

echo \"--- UDP (3s) ---\"
btest -s -P 19877 &
sleep 2
btest -c 127.0.0.1 -P 19877 -r -u -d 3
kill %1 2>/dev/null; wait 2>/dev/null || true

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
