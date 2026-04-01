#!/usr/bin/env bash
# build-rpm.sh — Build the btest-rs RPM package
set -euo pipefail

SPEC_DIR="$(cd "$(dirname "$0")" && pwd)"
SPEC_FILE="${SPEC_DIR}/btest-rs.spec"
VERSION="0.6.0"
TARBALL="v${VERSION}.tar.gz"
SOURCE_URL="https://github.com/manawenuz/btest-rs/archive/refs/tags/${TARBALL}"

echo "==> Setting up rpmbuild tree"
mkdir -p ~/rpmbuild/{BUILD,RPMS,SOURCES,SPECS,SRPMS}

echo "==> Downloading source tarball"
if [ ! -f ~/rpmbuild/SOURCES/"${TARBALL}" ]; then
    curl -fSL -o ~/rpmbuild/SOURCES/"${TARBALL}" "${SOURCE_URL}"
else
    echo "    (already present, skipping download)"
fi

echo "==> Copying spec file"
cp "${SPEC_FILE}" ~/rpmbuild/SPECS/btest-rs.spec

echo "==> Building RPM"
rpmbuild -ba ~/rpmbuild/SPECS/btest-rs.spec

echo ""
echo "==> Build complete. Packages:"
find ~/rpmbuild/RPMS -name '*.rpm' -print
find ~/rpmbuild/SRPMS -name '*.rpm' -print
