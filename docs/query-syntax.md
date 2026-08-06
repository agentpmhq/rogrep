# Query syntax

One grammar powers `rogrep search` (and the bare `rogrep QUERY` form),
`rogrep x`, `rogrep find`, `rogrep trajectory`, and the TUI search box.
A query is a whitespace-separated list of tokens; every token adds an
AND-ed constraint. There is no OR and no negation.

    rogrep 'flaky tokenizer tool_cmd:cargo since:30d'
    rogrep '/panic!\(/ project:rogrep'
    rogrep x 'tool_status:failed tool_type:tests since:7d'

A test (`skill.rs::facet_docs_stay_in_sync`) keeps this document, the
bundled SKILL.md, and `KNOWN_FACET_KEYS` in `crates/rogrep-index/src/query.rs`
in lockstep: every backticked `` `key:` `` here must be a real facet key,
and every real key must be documented here.

## Tokenization

- Tokens split on whitespace. Double quotes group: `"exact phrase"` is one
  token. An unterminated quote is flushed as a quoted token, never dropped.
- A quoted token is always literal text — `"tool:bash"` searches for the
  string `tool:bash`, it never activates a facet; `"/x/"` is a phrase, not
  a regex.
- `key:value` activates a facet **only** when `key` is a known facet key
  (dashes and underscores in the key are interchangeable:
  `tool-status:` == `tool_status:`). Anything else with a colon — URLs,
  `data:` URIs, timestamps — is a literal term, never silently dropped.
- A query that is exactly one `rg_…` conversation id (or `rg_…#eN`
  exchange ref) short-circuits to `rogrep show`. Pass `--` first
  (`rogrep -- rg_…`) to search for an id-shaped token as literal text.

## Terms and phrases

- Bare terms are lowercased, trimmed of surrounding punctuation, and
  dropped when shorter than 2 characters. All terms must match (AND), each
  within a single turn for a strict hit.
- `"quoted phrases"` require their words adjacent and in order.
- Matching is case-insensitive and token-based (the index splits text on
  punctuation), with BM25 relevance ranking plus a 30-day-half-life
  recency decay (`--sort recent` for pure recency).

## Regexes (rogrep extension)

An unquoted token that starts and ends with `/` is a regular expression
matched against **full turn text** — it crosses word boundaries and
punctuation, which terms cannot:

    rogrep '/RegexQuery::from_pattern/'
    rogrep '/error\s+CS-\d+/'
    rogrep '/(?i)segfault/ provider:codex'

- Full [regex crate](https://docs.rs/regex) syntax. Case-**sensitive** by
  default (terms are not); use an inline `(?i)` for case-insensitive.
- A pattern cannot contain spaces (tokenization is whitespace-first) —
  write `\s+`. Escaped slashes work: `/src\/lib/`.
- An invalid pattern is an error (with the pattern named), not a silent
  no-match. `/foo` (unterminated) and `//` fall back to literal terms.
- Regexes post-filter stored turn text. Combined with terms or facets they
  only filter those candidates (fast). A regex-**only** query scans the
  20,000 most-recent turns, newest first, and prints a note when older
  turns were left unscanned — add a term or facet to narrow.
- agentpm has no user-facing regex; this is a rogrep extension.

## Facets

`key:value` filters. Values may be:

- plain (`tool_cmd:git`),
- globs — `*` (any run, crosses `/`) and `?` (one char): `file:*_test.rs`,
- regexes — `/pattern/`, anchored to the whole indexed value
  (`tool_cmd:/carg./` matches `cargo`; use `.*` for substring effects).
  Facet regexes run on tantivy's regex engine (no look-around or
  backreferences) against lowercased values.

A leading `@` in a value is stripped (`project:@rogrep` == `project:rogrep`).
Repeating a key ANDs; there are no comma lists.

### Metadata facets — case-insensitive **substring** match

| Key | Matches against | Example |
|---|---|---|
| `provider:` / `agent:` | agent kind: claude, codex, cursor, grok, hermes, opencode | `provider:codex` |
| `model:` | model id (turn-level, falling back to the conversation's) | `model:sonnet` matches `claude-sonnet-4-5` |
| `project:` | normalized project key | `project:rogrep` |
| `cwd:` | working directory (turn-level, falling back to the conversation's) | `cwd:src/rogrep` |
| `file:` | absolute paths touched by tool calls | `file:src/lib.rs` |
| `source:` | rollout file path on disk | `source:.codex/sessions` |

Only conversations whose tool calls touched a file can match `file:`.

### Vocabulary facets — exact value match

| Key | Values | Notes |
|---|---|---|
| `role:` | user, assistant, tool, system, event | turn role |
| `origin:` | interactive, subagent, scheduled, auxiliary | conversation origin; naming `origin:auxiliary` opts auxiliary sessions into results |
| `subagent:` | true/1/yes/subagent, false/0/no/normal | sugar over `origin:` |
| `is:` | interrupted, compacted, notification, scheduled, subagent | turn/conversation states |
| `content:` | image | turns carrying an image |
| `tool:` | tool name, lowercased (`tool:bash`, `tool:mcp__posthog__query`) | |
| `skill:` | skill name from Skill tool calls | |
| `mcp:` | server part of `mcp__server__tool` | `mcp:posthog` |
| `tool_cmd:` | first executable of each shell segment | `tool_cmd:cargo`; pipe tails (`\| head`) are not commands |
| `tool_type:` | shell-command classification: issue-tracking, git-operation, git-inspection, git-push, git-pull, git-status, package-management, file-transfer, database-inspection, database-operation, tests, build, formatting, deployment, network-inspection, process-control, process-inspection, terminal-session, time-lookup, directory-inspection, inline-python-code, python-module, python-script, inline-node-code, inline-perl-code, inline-ruby-code, tool-version, project-script, search, http-request, service-operation, task-monitoring, cleanup, filesystem-update, file-inspection, generated-file, text-processing; non-shell tools use their slugged name (read-file, edit-file, grep, …) | `tool_type:tests` |
| `tool_location:` | local, remote (ssh/scp/rsync) | `tool_location:remote` |
| `tool_mutability:` | read-only, mutating | `tool_mutability:read-only` |
| `tool_privilege:` | privileged (sudo) — only emitted when privileged | |
| `tool_status:` | succeeded, failed, rejected, unknown | |
| `tool_mutating:` | true — only emitted for mutating git/gh commands | predates `tool_mutability:`; kept for compatibility |
| `git_cmd:` | git subcommand: status, diff, log, show, remote, rev-parse, ls-remote, merge-base, fetch, pull, push, add, commit, rm, restore, checkout, switch, branch, rebase, merge, cherry-pick, worktree, stash, reset, tag, clean | `git_cmd:push` |
| `git_pr:` | gh pr action: view, list, diff, checks, status, create, edit, merge, close, reopen, comment, review, ready, checkout — plus the composite `git_pr:create-num:N` (the PR a session created, mined from command output) | `git_pr:create` |
| `git_pr_num:` | PR number acted on | `git_pr_num:48` |
| `git_commit:` | commit sha, truncated to 7 chars on both sides | a 40-char sha in the query matches its short form |
| `git_branch:` | branch names from git/gh commands | slugged: lowercase, `_`→`-`, `/` and `.` kept |
| `git_remote:` | remote name from push/pull/fetch | `git_remote:origin` |

Vocabulary values normalize on both the index and query sides: lowercase,
`_`→`-` (`tool_mutability:read_only` == `tool_mutability:read-only`).

### Date facets

Resolved to a timestamp range, intersected with each other and with the
`--since` flag. Values are `YYYY-MM-DD` (local day boundary) or `Nd`
(N days before now).

| Key | Meaning |
|---|---|
| `since:` / `after:` | inclusive lower bound (start of day / now−Nd) |
| `before:` / `until:` | exclusive upper bound |
| `when:` | that one day (`when:Nd` behaves like `since:Nd`) |

An impossible combination (`since:2026-08-01 before:2026-07-01`) is an
error rather than an empty result.

## Default scope

Corpus-wide search (search/x/trajectory/TUI) covers **visible turns of
real work**: harness-injected context blocks and auxiliary sessions
(machine evaluation, e.g. codex auto-review judges) are excluded.
Conversation-scoped `rogrep find` greps everything, including injected
context. An explicit `origin:auxiliary` facet opts auxiliary sessions in.

## Deviations from agentpm

rogrep's grammar is a superset of agentpm's `conv search` with these
deliberate differences:

- `/regex/` (both bare and in facet values) is rogrep-only.
- `role:`, `origin:`, `subagent:` as a full truthy/falsey facet, the
  `is:` state vocabulary, and date facets in the query string are
  rogrep-only (agentpm passes dates as request parameters).
- Globs work in **all** facet values (agentpm: only `file:`/`cwd:`).
- Multi-tenant keys (`owner:`, `user:`, `agent_id:`, `tag:`) don't exist —
  rogrep is local-only.
- Value normalization applies `_`→`-` on both the index and query sides;
  agentpm's index side preserves `_` while its query side maps it, which
  can miss underscore-bearing values there.
