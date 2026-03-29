#!/usr/bin/env sh
set -eu

VERSION="$1"
OUTPUT="$2"

: "${DARWIN_ARM64_SHA256:?DARWIN_ARM64_SHA256 is required}"
: "${DARWIN_X64_SHA256:?DARWIN_X64_SHA256 is required}"
: "${LINUX_ARM64_SHA256:?LINUX_ARM64_SHA256 is required}"
: "${LINUX_X64_SHA256:?LINUX_X64_SHA256 is required}"

mkdir -p "$(dirname "$OUTPUT")"

cat >"$OUTPUT" <<EOF
class Domaingrep < Formula
  desc "Bulk domain availability search CLI tool"
  homepage "https://github.com/ysm-dev/domaingrep"
  version "${VERSION}"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/ysm-dev/domaingrep/releases/download/v${VERSION}/domaingrep-aarch64-apple-darwin.tar.gz"
      sha256 "${DARWIN_ARM64_SHA256}"
    else
      url "https://github.com/ysm-dev/domaingrep/releases/download/v${VERSION}/domaingrep-x86_64-apple-darwin.tar.gz"
      sha256 "${DARWIN_X64_SHA256}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/ysm-dev/domaingrep/releases/download/v${VERSION}/domaingrep-aarch64-unknown-linux-musl.tar.gz"
      sha256 "${LINUX_ARM64_SHA256}"
    else
      url "https://github.com/ysm-dev/domaingrep/releases/download/v${VERSION}/domaingrep-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${LINUX_X64_SHA256}"
    end
  end

  def install
    bin.install "domaingrep"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/domaingrep --version")
  end
end
EOF
