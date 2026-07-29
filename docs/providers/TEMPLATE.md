# Adding a provider

Checklist for provider #9 (each item is one file or one line):

1. `crates/rogrep-parsers/src/providers/<name>.rs` — implement `Provider`
   (kind, parser_version starting at 1, claims_path, source_info) and
   `RolloutParser` (process; finish/export_state/import_state only when the
   provider carries cross-record state). Start by delegating to
   `generic`-style extraction and specialize incrementally.
2. Register in `providers/mod.rs::REGISTRY` — order matters only when paths
   overlap (most specific first; see cowork-before-claude).
3. Add the `AgentKind` variant in `rogrep-model/src/conversation.rs`
   (including `ALL` and `parse`).
4. Discovery: if the provider has a fixed home root, add it to
   `discovery::provider_roots`; if it stores sessions in a database,
   implement an exporter in `spool/` following `hermes_db.rs` (per-session
   change fingerprint, skip-unchanged, atomic rewrite-on-change).
5. Fixtures: `fixtures/<name>/*.jsonl` — minimum: a basic session with tool
   pairing. Add entries to `tests/snapshots.rs` and
   `tests/incremental.rs::new_providers_all_line_splits` (the golden
   invariant test catches nearly all incremental bugs for free).
6. `docs/providers/<name>.md` — storage paths, record shapes, extracted
   fields, known gaps.
7. Run `cargo insta test -p rogrep-parsers` and review snapshots.

Semantics to preserve (the driver handles these when you emit correctly):

- Tool calls and results are both `Role::Tool` turns linked by `pair_id`;
  output status is stamped back onto the call automatically.
- Conversation cwd = FIRST record cwd (`ctx.set_cwd` every time; the driver
  does first-wins); per-turn cwd tracks drift.
- User-shaped harness records (notifications, scheduled prompts) must carry
  `special`/`synthetic_context` so they don't open exchanges.
- Provider-reported cumulative usage → delta in the parser, attach within
  the open exchange only (`ctx.amendable()`), pending-carry across records.
