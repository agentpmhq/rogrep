---
name: rogrep
description: Search and analyze local coding-agent session history (Claude Code, Codex, Cursor, Grok, Hermes, opencode) with rogrep. Use when you need to enumerate past agent work, search conversations, inspect exchanges or turns, review git/GitHub activity, compute usage statistics, or answer questions like "how did we get to PR 48?", "what led to this branch?", "what did we work on last week?", or "which sessions touched this file?".
---

# rogrep — local rollout search

`rogrep` indexes every coding-agent session on this machine (fully local; nothing leaves the box) and answers evidence questions about past work. Every command auto-refreshes the index first, so results are always current. Prefer concise text output while exploring; add `--json` when exact fields matter.

## Core concepts

- **Conversation** (`rg_…` id): one agent session.
- **Exchange** (`rg_…#eN`, 1-based): one real user prompt plus every agent action until the next prompt — the primary unit for "what did the agent do when I asked X". Harness echoes (task notifications, scheduled prompts, compactions) never open exchanges.
- **Turn** (index within a conversation): a single message, tool call, or tool result.

## Start Broad

- `rogrep ls` — recent conversations across all agents; `--cwd .` scopes to the current checkout, `--project KEY` to a normalized project key.
- `rogrep stats projects` — per-project activity (conversations, exchanges, tokens, last active).
- `rogrep stats daily` / `weekly` / `monthly` — usage tables; `rogrep stats heatmap` — hour-of-week activity; `rogrep stats top --by tokens|duration|turns|tools` — the biggest exchanges; `rogrep stats tools` — tool call/failure counts; `rogrep stats git` — daily commits/pushes/PR actions.

## Search And Drill In

- `rogrep search QUERY` (or bare `rogrep QUERY`) searches all turns. Include exact identifiers when known: PR number, branch, file path, error text. A bare `rg_…` id resolves directly; `rg_…#eN` shows that exchange.
- Query grammar: bare terms AND together; `"quoted phrases"` match in order; `key:value` facets filter. Unknown `key:` tokens (URLs, `data:` URIs) are treated as literal text — only known facet keys activate.
- Facets: `provider:` (claude|codex|cursor|grok|hermes|opencode), `model:`, `project:`, `cwd:`, `file:`, `role:`, `tool:` (tool name), `tool_cmd:` (shell executable, e.g. `tool_cmd:git`), `tool_status:` (succeeded|failed|rejected), `tool_mutating:true`, `git_cmd:` (commit|push|…), `git_pr:` (create|merge|…), `git_pr_num:N`, `git_commit:SHA`, `git_branch:`, `is:` (interrupted|compacted|notification), `since:`/`before:` (YYYY-MM-DD or Nd, e.g. `since:7d`).
- `rogrep find CONVERSATION_ID QUERY` — conjunction find inside one conversation. Reports strict turn hits first; when no single turn has every term, it reports exchanges where all terms appear ("passage" hits, with ready-to-run `rogrep show rg_…#eN` commands); per-term counts show which term narrowed the query.
- `rogrep show ID` — render turns. `--turn N` for one turn, `--around N --limit 40` for context, `rg_…#eN` or `--exchange N` for one exchange. Text output truncates long turns at ~2400 bytes; use `--json` or `--raw` for exact payloads.

## Interpret Missing Data

- "no matches" from search means nothing INDEXED matched — differently-phrased, unparsed, or out-of-band work is not ruled out. Retry with broader or alternative terms before concluding absence.
- An empty `rogrep git` timeline means no indexed git/GitHub facets matched. It does not rule out filesystem edits, shell work, or SSH work.
- Token totals labeled `est` are length-based estimates for turns with no provider accounting; some providers (cursor, grok) never report usage.
- `rogrep doctor` reports discovery roots, per-provider file counts, and parse health when results seem incomplete.

## Answer Trajectory Questions

For "how did we get to PR 48?":

1. `rogrep trajectory --pr 48` (add `--branch NAME`, `--commit SHA`, `--cwd .`, or a text query). The `[origin]` entry created the PR; entries are ranked by real git evidence and ordered chronologically.
2. `rogrep git CONVERSATION_ID --pr 48 --mutating-only` for the git timeline of a candidate.
3. Follow each result's `inspect:` line (`rogrep show rg_… --around N --limit 40`); in `--json` the same command is in the `inspect` field with `next_turn`.
4. Search again for missing evidence: `pull/48`, the branch name, `git_pr:create`, `git_pr_num:48`, commit title, or feature wording.
5. Answer as a chronology grounded in evidence: conversation ids, exchange refs, turn indexes, branch names, PR URLs. Say when evidence is partial instead of filling gaps.

## Output Discipline

- Do not claim causality unless turns or git operations support it.
- Prefer short quoted snippets; paraphrase the rest.
- Cite `rg_…` ids with turn indexes or `#eN` exchange refs so another agent can reproduce the path.
- Rank competing trajectories by direct evidence: PR URL > branch name > exact title/text > broad keyword match.
- Never run `rogrep tui` from a non-interactive session; it is for humans.
