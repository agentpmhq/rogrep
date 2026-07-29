# rogrep — rollout grep

Local search, statistics, and trajectory analysis over your coding-agent
sessions. rogrep indexes every rollout on your machine — **Claude Code,
Codex, Cursor, Grok, Hermes, opencode** — and answers questions like:

- *"How did we get to PR 48?"* → `rogrep trajectory --pr 48`
- *"Which sessions had failing cargo runs?"* → `rogrep tool_status:failed tool_cmd:cargo`
- *"What was my most expensive request this week?"* → `rogrep stats top --by tokens --since 7d`
- *"What did the agent do when I asked it to fix the parser?"* → `rogrep x "fix the parser"`

Everything is local. No daemon, no server, no telemetry — **no data ever
leaves your machine.** One binary, powered by [tantivy](https://github.com/quickwit-oss/tantivy)
(sub-millisecond full-text search) and SQLite (deterministic statistics).

## Install

```sh
cargo install --path crates/rogrep      # from a checkout
rogrep skill install                    # optional: teach local agents to use rogrep
```

The index lives in `~/.local/share/rogrep` and refreshes incrementally at
the start of every command — a steady-state refresh costs milliseconds; the
first run indexes your whole history (about 20s per GB).

## Concepts

- **Conversation** (`rg_…`): one agent session, discovered from each
  provider's on-disk sessions.
- **Exchange** (`rg_…#eN`): one real user prompt plus *everything the agent
  did in response* — the unit you usually care about. Harness noise (task
  notifications, scheduled prompts, compactions) never splits exchanges.
- **Turn**: a single message, tool call, or tool result.

## Commands

```
rogrep QUERY                  search everything (alias for rogrep search)
rogrep x QUERY [--failed --min-duration 5m ...]   search/list exchanges
rogrep find CONV QUERY        conjunction find inside one conversation
rogrep show CONV[#eN]         render turns / one exchange (--json for exact payloads)
rogrep git CONV [--pr N]      git/GitHub timeline of a conversation
rogrep trajectory --pr N      which conversations led to a PR/branch/commit
rogrep ls / stats / doctor    listings, usage reports, health checks
rogrep tui                    interactive terminal UI
```

Query grammar: bare terms AND together, `"quoted phrases"` match in order,
and `key:value` facets filter — `tool_cmd:git`, `tool_status:failed`,
`git_pr_num:48`, `provider:codex`, `file:src/lib.rs`, `since:7d`, and more
(`rogrep skill show` documents the full set). Tokens with unknown keys
(URLs, `data:` URIs) are treated as literal text, never silently dropped.

## Statistics

Deterministic, local, no LLM:

```
rogrep stats daily|weekly|monthly    usage tables (conversations, exchanges, tokens)
rogrep stats heatmap                 hour-of-week activity grid
rogrep stats top --by tokens         biggest exchanges (also duration/turns/tools)
rogrep stats tools                   tool call/failure counts, top shell commands
rogrep stats git                     daily commits / pushes / PR actions
rogrep stats projects                per-project rollups
```

## Architecture (for contributors)

Cargo workspace: `rogrep-model` (normalized types) → `rogrep-parsers`
(provider parsers + discovery + SQLite spool exporters) → `rogrep-tooltree`
(shell/git facet extraction) → `rogrep-store` (SQLite) + `rogrep-index`
(tantivy) → `rogrep-engine` (sync pipeline) → `rogrep` (CLI/TUI).

Design invariants worth knowing:

- **Exact incremental parsing.** Checkpoints freeze at the start of the last
  exchange; late tool results and usage records may only amend the open
  exchange. `parse(full) == parse(prefix) + resume(tail)` holds exactly and
  is tested at every line boundary of every fixture.
- **Everything derived is disposable.** Parser/schema version bumps wipe and
  rebuild the affected slice automatically; source rollout files are the
  only durable truth.
- **One tantivy index**, one document per turn (text stored for sub-ms
  excerpts), exchange grouping via fast fields; tail refresh =
  delete-from-watermark + re-add, idempotent under crashes.

Adding a provider = one module in `rogrep-parsers/src/providers/`, a
registry entry, an `AgentKind` variant, and fixtures with snapshot tests.

## License

MIT OR Apache-2.0.
