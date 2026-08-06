---
layout: docs.njk
title: Agent setup and playbook
description: Teach coding agents to recover context and cite local rogrep evidence.
---
## Install the bundled skill

~~~sh
rogrep skill install
rogrep skill show
rogrep skill uninstall
~~~

Installation always targets `~/.agents/skills/rogrep/SKILL.md`. When `~/.claude`, `~/.codex`, or `~/.grok` exists, rogrep also installs in that agent's `skills/rogrep/` directory. [Read the rendered skill](/docs/agents/skill/) or [raw source](https://raw.githubusercontent.com/agentpmhq/rogrep/main/crates/rogrep/assets/SKILL.md).

## AGENTS.md snippet

~~~md
## Local conversation history (rogrep)
Use rogrep before debugging or rebuilding work a previous coding-agent session may already have investigated. Start with `rogrep QUERY` or `rogrep x QUERY`; narrow with `rogrep find CONVERSATION_ID QUERY`, then inspect with `rogrep show`, `rogrep git`, or `rogrep trajectory --pr N`. Use `--json` for automation and text for exploration. Cite conversation IDs, exchange references, and turn indexes. Do not launch `rogrep tui` from a non-interactive session.
~~~

## Search, then drill down

1. Recover context: `rogrep x "incremental parser"`.
2. Locate exact turns: `rogrep find rg_& "checkpoint"`.
3. Read the exchange: `rogrep show rg&#e4` or `rogrep show --around 27 rg&`.
4. For a PR or branch, run `rogrep trajectory --pr 48` and inspect candidates with `rogrep git CONVERSATION_ID`.

For failures, start with `rogrep tool_status:failed tool_cmd:cargo`. For file history, start with `rogrep file:src/lib.rs`. A tool failure without its surrounding request and repair attempt is incomplete evidence.

## Output, evidence, and limits

Use text while exploring and `--json` for automation. Cite `rg&` IDs, `rg&#eN` exchanges, and turn indexes. Prefer captured Git operations over text mentions when claiming causality.

An empty timeline is not proof nothing happened. Metadata or source records may be incomplete. Default corpus search excludes invisible harness turns and auxiliary conversations; conversation-scoped `find` searches everything. State those limits. `rogrep tui` requires an interactive terminal.
