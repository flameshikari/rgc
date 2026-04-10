#!/bin/sh
set -e

REPO="flameshikari/rgc"
# Find existing bin directory
if [ -d "/usr/bin" ]; then
    DEST="/usr/bin"
elif [ -d "/usr/local/bin" ]; then
    DEST="/usr/local/bin"
elif [ -d "/usr/local/sbin" ]; then
    DEST="/usr/local/sbin"
else
    echo "error: no suitable bin directory found" >&2
    exit 1
fi

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
    x86_64)  ARCH="amd64" ;;
    aarch64) ARCH="arm64" ;;
    *) echo "error: unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

# Get latest release tag
TAG=$(curl -sfL "https://api.github.com/repos/${REPO}/releases/latest" | grep -o '"tag_name": *"[^"]*"' | cut -d'"' -f4)
if [ -z "$TAG" ]; then
    echo "error: could not determine latest version" >&2
    exit 1
fi

ASSET="rgc-${TAG}-${ARCH}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"

if [ -f "${DEST}/rgc" ]; then
    CURRENT=$("${DEST}/rgc" -v 2>/dev/null || echo "unknown")
    echo "rgc ${CURRENT} already installed in ${DEST}, use 'rgc --update' instead"
    exit 0
fi

echo "downloading ${ASSET}..."
curl -sfL "$URL" | tar xz -C "$DEST"
chmod +x "${DEST}/rgc"

VERSION=$("${DEST}/rgc" -v 2>/dev/null || echo "$TAG")
echo "rgc ${VERSION} installed to ${DEST}/rgc"
