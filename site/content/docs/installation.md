---
layout: docs.njk
title: Installation
description: Install rogrep from a release archive or source.
---
## Release archives

[GitHub releases](https://github.com/agentpmhq/rogrep/releases) provide tarballs and SHA-256 checksums for Linux and macOS. Download your platform archive, verify its checksum, and put `rogrep` on `PATH`.

## Repository installer

~~~sh
git clone https://github.com/agentpmhq/rogrep
cd rogrep
./scripts/install.sh
~~~

This installs a release build at `~/.local/bin/rogrep` and the agent skill. It does not use root.

## Cargo

Rust 1.85 or newer is required.

~~~sh
cargo install --path crates/rogrep
rogrep skill install
~~~

Continue with [your first sync](/docs/getting-started/).
