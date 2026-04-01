#!/usr/bin/env bash
# Test the PKGBUILD in a Docker Arch Linux container.
# Usage: ./deploy/aur/test-aur.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

echo "=== Testing AUR PKGBUILD in Arch Linux container ==="

docker run --rm -v "$(pwd):/src:ro" archlinux:latest bash -c '
    set -euo pipefail

    # Install base-devel and rust
    pacman -Syu --noconfirm base-devel rustup git
    rustup default stable

    # Create build user (makepkg refuses to run as root)
    useradd -m builder
    echo "builder ALL=(ALL) NOPASSWD: ALL" >> /etc/sudoers

    # Copy source and PKGBUILD
    su builder -c "
        mkdir -p /tmp/build && cd /tmp/build
        cp /src/deploy/aur/PKGBUILD .

        # Build the package
        makepkg -si --noconfirm

        # Verify
        echo ''
        echo '=== Installed ==='
        btest --version
        btest --help | head -5
        echo ''
        echo '=== Files ==='
        pacman -Ql btest-rs
        echo ''
        echo '=== SUCCESS ==='
    "
'
