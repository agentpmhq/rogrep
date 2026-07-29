#!/usr/bin/env bash
# rogrep installer: builds from source (until binary releases exist) and
# installs to ~/.local/bin. Everything rogrep does stays on this machine.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p rogrep
mkdir -p "$HOME/.local/bin"
install -m 0755 target/release/rogrep "$HOME/.local/bin/rogrep"
echo "installed $HOME/.local/bin/rogrep"
"$HOME/.local/bin/rogrep" skill install || true
echo "run 'rogrep sync' to build the index, then 'rogrep --help'"
