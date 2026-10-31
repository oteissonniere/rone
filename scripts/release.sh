#!/usr/bin/env bash
# Build rone and package a distributable tarball (binary + presets + docs).
#
# Usage: scripts/release.sh [target-triple]
#   target-triple is optional; defaults to this machine's host triple, e.g.
#     aarch64-apple-darwin         Apple Silicon
#     x86_64-apple-darwin          Intel Mac
#     aarch64-unknown-linux-gnu    ARM Linux
#     x86_64-unknown-linux-gnu     64-bit Linux (glibc)
#   The target must already be installed (rustup target add <triple>).
#
# Output:
#   dist/rone-v<version>-<target>.tar.gz         the package
#   dist/rone-v<version>-<target>.tar.gz.sha256  SHA-256 sidecar

set -euo pipefail

cd "$(dirname "$0")/.."

target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

if [ -z "${version}" ]; then
  echo "error: could not read the package version from Cargo.toml" >&2
  exit 1
fi

stem="rone-v${version}-${target}"
tarball="dist/${stem}.tar.gz"

stage="$(mktemp -d)"
trap 'rm -rf "${stage}"' EXIT

echo ">> cargo build --release --target ${target}"
cargo build --release --target "${target}"

mkdir -p dist "${stage}/${stem}"
cp "target/${target}/release/rone"     "${stage}/${stem}/rone"
cp -R presets                           "${stage}/${stem}/presets"
cp README.md LICENSE CHANGELOG.md      "${stage}/${stem}/"

echo ">> tar czf ${tarball}"
tar -czf "${tarball}" -C "${stage}" "${stem}"

if command -v sha256sum >/dev/null 2>&1; then
  (cd dist && sha256sum "${stem}.tar.gz" > "${stem}.tar.gz.sha256")
else
  (cd dist && shasum -a 256 "${stem}.tar.gz" > "${stem}.tar.gz.sha256")
fi

echo ">> done: ${tarball} ($(du -h "${tarball}" | awk '{print $1}'))"
