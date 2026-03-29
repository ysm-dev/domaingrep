#!/usr/bin/env sh
set -eu

REPO="ysm-dev/domaingrep"
VERSION="${DOMAINGREP_VERSION:-latest}"
INSTALL_DIR="${HOME}/.domaingrep/bin"
TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "$TMP_DIR"
}

trap cleanup EXIT INT TERM

uname_s="$(uname -s)"
uname_m="$(uname -m)"

case "$uname_s" in
  Darwin)
    case "$uname_m" in
      arm64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) echo "error: unsupported macOS architecture '$uname_m'" >&2; exit 1 ;;
    esac
    ;;
  Linux)
    case "$uname_m" in
      x86_64) target="x86_64-unknown-linux-musl" ;;
      aarch64|arm64) target="aarch64-unknown-linux-musl" ;;
      *) echo "error: unsupported Linux architecture '$uname_m'" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "error: unsupported operating system '$uname_s'" >&2
    exit 1
    ;;
esac

if [ "$VERSION" = "latest" ]; then
  asset_url="https://github.com/${REPO}/releases/latest/download/domaingrep-${target}.tar.gz"
else
  asset_url="https://github.com/${REPO}/releases/download/${VERSION}/domaingrep-${target}.tar.gz"
fi

mkdir -p "$INSTALL_DIR"
curl -fsSL "$asset_url" -o "$TMP_DIR/domaingrep.tar.gz"
tar -xzf "$TMP_DIR/domaingrep.tar.gz" -C "$TMP_DIR"
install "$TMP_DIR/domaingrep" "$INSTALL_DIR/domaingrep"

printf '%s\n' "domaingrep was installed to ${INSTALL_DIR}/domaingrep"
printf '%s\n' 'Add the following to your shell profile:'
printf '%s\n' '  export PATH="$HOME/.domaingrep/bin:$PATH"'
