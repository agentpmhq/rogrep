# Contributing to rogrep

## Development workflow

`main` is protected: every change lands through a pull request (squash or
merge commit; force pushes and branch deletion are blocked, and merging is
restricted to the maintainer).

1. Branch from `main`: `git checkout -b my-change`.
2. Make the change with tests (see below).
3. Push and open a PR: `gh pr create --fill`.
4. CI must pass — it builds the workspace with `-D warnings` (warnings are
   errors) and runs the full test suite.
5. Squash-merge; head branches auto-delete.

## Building and testing

```sh
cargo build --workspace --all-targets   # must be warning-free
cargo test --workspace                  # full suite
cargo insta review                      # review parser snapshot changes
```

Parser behavior is locked by [insta](https://insta.rs) snapshots under
`crates/rogrep-parsers/tests/snapshots/`. If your change intentionally
alters parse output, run `cargo insta review` and commit the accepted
snapshots; unexplained snapshot churn in a PR is a red flag.

The load-bearing test is the **incremental invariant**
(`crates/rogrep-parsers/tests/incremental.rs`):
`parse(full) == parse(prefix) + resume(tail)` at every line boundary,
including full checkpoint-state equality. If you touch the driver, a
parser, or `ParseState`, this is the test that keeps you honest. See
`docs/providers/TEMPLATE.md` for adding a provider.

## Version-bump discipline

Everything derived (SQLite store, tantivy index, checkpoints) rebuilds
automatically from source rollout files when versions change — there are no
migrations. Bump the right constant with your change:

| You changed… | Bump | Effect on users' next sync |
|---|---|---|
| One provider's parse output | that provider's `*_PARSER_VERSION` | re-derives just that provider's files |
| Turn/facet semantics affecting all providers | the relevant parser versions | re-derive of those providers |
| The tantivy schema or what gets indexed | `INDEX_SCHEMA_VERSION` (`rogrep-index/src/schema.rs`) | fresh index dir + full re-derive (via the `Indexer::generation()` handshake) |
| SQLite tables/columns | `SCHEMA_VERSION` (`rogrep-store/src/schema.rs`) | store wiped and re-derived |

Forgetting a bump means existing users keep stale derived data — the CI
tests can't catch that, so reviewers should check for it explicitly.

## Releasing

```sh
git tag vX.Y.Z -m "rogrep X.Y.Z"
git push origin vX.Y.Z
```

The `release` workflow builds `x86_64`/`aarch64` linux-musl (fully static)
and both macOS targets, packages tarballs + sha256 checksums, and publishes
a GitHub release with generated notes. Bump the workspace `version` in
`Cargo.toml` (and let `Cargo.lock` update) in a PR before tagging.

## Benchmarks

`scripts/bench-vs-cass.sh` reproduces the comparison in
`docs/benchmarks/`. Benchmarks index into throwaway data dirs and never
touch your real index. If a change plausibly affects indexing or query
performance, re-run it and say so in the PR.

## Conduct & scope

rogrep is local-only by design: no telemetry, no network calls outside the
explicitly-disabled remote-analysis seam (`rogrep-model/src/remote.rs`).
PRs that add network I/O anywhere else will be declined.
