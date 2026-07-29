#!/usr/bin/env bash
# Build release binaries for distribution. Requires the appropriate rust
# targets (`rustup target add x86_64-unknown-linux-musl` etc. for cross
# builds); by default builds the host target only.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
mkdir -p dist
cargo build --release -p rogrep
HOST=$(rustc -vV | awk '/^host:/ {print $2}')
cp target/release/rogrep "dist/rogrep-${VERSION}-${HOST}"
echo "built dist/rogrep-${VERSION}-${HOST}"
