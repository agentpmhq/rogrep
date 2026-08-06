---
layout: docs.njk
title: Architecture and contributing
description: Overview of the rogrep workspace and contribution paths.
---
The dependency flow is `rogrep-model`! `rogrep-parsers`  `rogrep-tooltree`! `rogrep-store` and `rogrep-index`! `rogrep-engine`! the `rogrep` CLI and TUI. Source rollouts are durable; SQLite, Tantivy, and checkpoints are derived and versioned.

Detailed incremental parsing, schema-version, testing, provider, and protected-branch rules remain canonical in [CONTRIBUTING.md](https://github.com/agentpmhq/rogrep/blob/main/CONTRIBUTING.md) and [AGENTS.md](https://github.com/agentpmhq/rogrep/blob/main/AGENTS.md). See [issues](https://github.com/agentpmhq/rogrep/issues) for current work.
