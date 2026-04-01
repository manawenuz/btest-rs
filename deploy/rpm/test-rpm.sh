#!/usr/bin/env bash
# test-rpm.sh — Test the btest-rs RPM build inside a Fedora container
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

IMAGE="fedora:latest"

echo "==> Testing RPM build in ${IMAGE}"
docker run --rm \
    -v "${REPO_ROOT}:/workspace:ro" \
    "${IMAGE}" \
    bash -euxc '
        # ── Install build dependencies ──
        dnf install -y rpm-build rpmdevtools curl gcc make \
            systemd-rpm-macros

        # Install Rust toolchain
        curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --profile minimal
        source "$HOME/.cargo/env"

        # ── Set up rpmbuild tree ──
        rpmdev-setuptree

        VERSION="0.6.0"
        TARBALL="v${VERSION}.tar.gz"

        # Copy spec
        cp /workspace/deploy/rpm/btest-rs.spec ~/rpmbuild/SPECS/

        # Create source tarball from workspace
        # rpmbuild expects btest-rs-VERSION/ top-level directory
        mkdir -p /tmp/btest-rs-${VERSION}
        cp -a /workspace/. /tmp/btest-rs-${VERSION}/
        tar czf ~/rpmbuild/SOURCES/${TARBALL} -C /tmp btest-rs-${VERSION}

        # ── Build RPM ──
        rpmbuild -ba ~/rpmbuild/SPECS/btest-rs.spec

        # ── Install the RPM ──
        RPM=$(find ~/rpmbuild/RPMS -name "btest-rs-*.rpm" | head -1)
        echo "Installing: ${RPM}"
        dnf install -y "${RPM}"

        # ── Verify installation ──
        echo "--- btest --version ---"
        btest --version

        echo "--- Checking systemd unit ---"
        systemctl cat btest.service || true

        echo "--- Checking man page ---"
        test -f /usr/share/man/man1/btest.1* && echo "man page OK" || echo "man page MISSING"

        echo "--- Checking license ---"
        test -f /usr/share/licenses/btest-rs/LICENSE && echo "license OK" || echo "license MISSING"

        # ── Loopback bandwidth test ──
        echo "--- Starting loopback test ---"
        btest -s &
        SERVER_PID=$!
        sleep 2

        btest -c 127.0.0.1 --duration 3 && echo "Loopback test PASSED" \
            || echo "Loopback test FAILED (exit $?)"

        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true

        echo "==> All RPM tests completed."
    '

echo "==> Fedora container test finished."
