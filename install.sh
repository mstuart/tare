#!/bin/sh
# tare installer — downloads the prebuilt, self-contained binaries (tare, tare-proxy, tare-mcp) for
# this platform from a GitHub Release. No Rust toolchain required.
#
#   curl -fsSL https://raw.githubusercontent.com/mstuart/tare/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --dir /usr/local/bin
#
# Env overrides: TARE_VERSION, TARE_INSTALL_DIR, TARE_DOWNLOAD_BASE (mirror/local base URL).
set -eu

REPO="mstuart/tare"
VERSION="${TARE_VERSION:-latest}"
INSTALL_DIR="${TARE_INSTALL_DIR:-$HOME/.local/bin}"
BASE="${TARE_DOWNLOAD_BASE:-}"

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --dir) INSTALL_DIR="$2"; shift 2 ;;
    *) echo "tare install: unknown argument: $1" >&2; exit 1 ;;
  esac
done

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) os_t="apple-darwin" ;;
  Linux) os_t="unknown-linux-gnu" ;;
  *) echo "tare: unsupported OS '$os' — try the npm package or build from source." >&2; exit 1 ;;
esac
case "$arch" in
  arm64 | aarch64) arch_t="aarch64" ;;
  x86_64 | amd64) arch_t="x86_64" ;;
  *) echo "tare: unsupported architecture '$arch'." >&2; exit 1 ;;
esac
target="${arch_t}-${os_t}"

if [ "$VERSION" = "latest" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)"
  [ -n "$VERSION" ] || { echo "tare: could not resolve the latest release." >&2; exit 1; }
fi

asset="tare-${target}.tar.gz"
url="${BASE:-https://github.com/$REPO/releases/download/$VERSION}/$asset"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "tare: downloading $asset ($VERSION)…"
curl -fsSL "$url" -o "$tmp/$asset"

# Verify the checksum if one is published alongside the asset.
if curl -fsSL "${url}.sha256" -o "$tmp/$asset.sha256" 2>/dev/null; then
  ( cd "$tmp" && { sha256sum -c "$asset.sha256" >/dev/null 2>&1 \
      || shasum -a 256 -c "$asset.sha256" >/dev/null 2>&1; } ) \
    || { echo "tare: checksum verification failed." >&2; exit 1; }
fi

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
for b in tare tare-proxy tare-mcp; do
  install -m 0755 "$tmp/$b" "$INSTALL_DIR/$b"
done

echo "tare: installed tare, tare-proxy, tare-mcp to $INSTALL_DIR"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "tare: add $INSTALL_DIR to your PATH to use them." ;;
esac
