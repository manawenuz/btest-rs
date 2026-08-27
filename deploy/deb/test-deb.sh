#!/usr/bin/env bash
# test-deb.sh -- Smoke-test a btest-rs .deb inside an Ubuntu Docker container
#
# Usage:
#   ./deploy/deb/test-deb.sh                       # auto-finds dist/*.deb
#   ./deploy/deb/test-deb.sh path/to/btest-rs_*.deb
#
# Requirements: docker
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
IMAGE="${TEST_IMAGE:-ubuntu:24.04}"

###############################################################################
# Locate the .deb
###############################################################################
if [[ -n "${1:-}" ]]; then
    DEB_PATH="$1"
else
    DEB_PATH="$(ls -1t "$REPO_ROOT"/dist/btest-rs_*.deb 2>/dev/null | head -1 || true)"
fi

if [[ -z "$DEB_PATH" || ! -f "$DEB_PATH" ]]; then
    echo "Error: no .deb file found."
    echo "  Build first:  ./deploy/deb/build-deb.sh"
    echo "  Or pass path:  $0 path/to/btest-rs_*.deb"
    exit 1
fi

DEB_FILE="$(basename "$DEB_PATH")"
DEB_DIR="$(cd "$(dirname "$DEB_PATH")" && pwd)"

echo "==> Testing $DEB_FILE in $IMAGE"
echo ""

###############################################################################
# Run tests inside a disposable container
###############################################################################
docker run --rm \
    -v "$DEB_DIR/$DEB_FILE:/tmp/$DEB_FILE:ro" \
    "$IMAGE" \
    bash -euxc "
        ###################################################################
        # 1. Install the .deb
        ###################################################################
        apt-get update -qq
        dpkg -i /tmp/$DEB_FILE || apt-get install -f -y  # resolve deps if any

        ###################################################################
        # 2. Verify files are in place
        ###################################################################
        echo '--- Checking installed files ---'
        test -x /usr/bin/btest
        test -f /usr/lib/systemd/system/btest.service
        grep -q -- '--listen6' /usr/lib/systemd/system/btest.service
        # Ubuntu's container image excludes most /usr/share/doc files while
        # unpacking. Verify README.md in the package payload instead.
        dpkg-deb -c /tmp/$DEB_FILE > /tmp/btest-package.contents
        grep -q './usr/share/doc/btest-rs/README.md' /tmp/btest-package.contents
        grep -q './usr/share/man/man1/btest.1.gz' /tmp/btest-package.contents
        test -f /usr/share/licenses/btest-rs/LICENSE
        echo 'All expected files present.'

        ###################################################################
        # 3. btest --version
        ###################################################################
        echo ''
        echo '--- btest --version ---'
        btest --version

        ###################################################################
        # 4. Quick loopback server+client test
        ###################################################################
        echo ''
        echo '--- Loopback smoke test ---'

        # Start server in background
        btest -s --listen6 &
        SERVER_PID=\$!
        sleep 1

        # Run a short TCP test against localhost
        if btest -c 127.0.0.1 -t -d 2 2>&1; then
            echo 'Loopback TCP test passed.'
        else
            echo 'IPv4 loopback TCP test failed.' >&2
            exit 1
        fi

        if btest -c ::1 -t -d 2 2>&1; then
            echo 'IPv6 loopback TCP test passed.'
        else
            echo 'IPv6 loopback TCP test failed.' >&2
            exit 1
        fi

        # Tear down
        kill \$SERVER_PID 2>/dev/null || true
        wait \$SERVER_PID 2>/dev/null || true

        ###################################################################
        # 5. Package metadata sanity
        ###################################################################
        echo ''
        echo '--- dpkg metadata ---'
        dpkg -s btest-rs | head -20

        echo ''
        echo '=== All tests passed ==='
    "

echo ""
echo "==> .deb smoke test completed successfully."
