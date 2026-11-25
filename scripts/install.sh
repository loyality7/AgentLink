#!/usr/bin/env sh
set -eu

REPO="${AGENTLINK_REPO:-loyality7/AgentLink}"
INSTALL_DIR="${HOME}/.local/bin"
mkdir -p "$INSTALL_DIR"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}:${ARCH}" in
  Darwin:arm64)  TARGET="aarch64-apple-darwin" ;;
  Darwin:x86_64) TARGET="x86_64-apple-darwin" ;;
  Linux:x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
  Linux:aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
  *) echo "Unsupported platform: ${OS} ${ARCH}" >&2; exit 1 ;;
esac

URL="https://github.com/${REPO}/releases/latest/download/agentlink-${TARGET}.tar.gz"
echo "Downloading AgentLink from ${URL}..."
curl -fsSL "$URL" | tar -xz -C "$INSTALL_DIR"
chmod +x "${INSTALL_DIR}/agentlink"

echo "Installed agentlink to ${INSTALL_DIR}/agentlink"
echo "Next: run 'agentlink init --name <your-name>' and 'agentlink setup'"

