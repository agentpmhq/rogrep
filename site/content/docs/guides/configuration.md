---
layout: docs.njk
title: Configuration and data paths
description: Configure rogrep local paths.
---
Derived data defaults to `~/.local/share/rogrep` and configuration to `~/.config/rogrep/config.toml`. Set `ROGREP_DATA_DIR` to relocate derived indexes. Run `rogrep doctor` after changing discovery or paths. Source rollout stores are read-only and remain the durable truth.
