#!/usr/bin/env bash
# Benchmark rogrep against cass (github.com/Dicklesworthstone/coding_agent_session_search)
# on THIS machine's real agent-session corpus.
#
# Both tools index into throwaway data dirs (your real indexes are untouched)
# and read the same on-disk sessions. cass runs in its default out-of-the-box
# configuration, which is lexical-only search (SQLite FTS5) — semantic assets
# are opt-in and never auto-downloaded — so this compares lexical search
# engines: tantivy (rogrep) vs SQLite FTS5 (cass).
#
# Usage:
#   CASS_BIN=/path/to/cass ./scripts/bench-vs-cass.sh [reps]
#
# Requires: bash, /usr/bin/time-style wall clocks via date +%s%N (GNU date).
set -euo pipefail

REPS="${1:-10}"
CASS_BIN="${CASS_BIN:?set CASS_BIN to a release cass binary}"
ROGREP_BIN="${ROGREP_BIN:-$(dirname "$0")/../target/release/rogrep}"
WORK="$(mktemp -d /tmp/rogrep-bench.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

export ROGREP_DATA_DIR="$WORK/rogrep-data"
export CASS_DATA_DIR="$WORK/cass-data"
export NO_COLOR=1

now_ms() { date +%s%N | awk '{printf "%d", $1/1000000}'; }

# time_cmd NAME -- cmd args…  → prints elapsed ms, appends to $TIMES
time_cmd() {
  local start end
  start=$(now_ms)
  "$@" >/dev/null 2>&1 || true
  end=$(now_ms)
  echo $((end - start))
}

median() {
  printf '%s\n' "$@" | sort -n | awk '{a[NR]=$1} END {print (NR%2) ? a[(NR+1)/2] : int((a[NR/2]+a[NR/2+1])/2)}'
}

run_reps() { # run_reps N -- cmd…  → "min median max" in ms
  local n=$1; shift; shift # drop N and --
  local times=()
  for _ in $(seq 1 "$n"); do
    times+=("$(time_cmd "$@")")
  done
  local sorted
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  echo "$(echo "$sorted" | head -1) $(median "${times[@]}") $(echo "$sorted" | tail -1)"
}

echo "== versions"
"$ROGREP_BIN" --version
"$CASS_BIN" --version || true
echo "reps per query: $REPS"
echo

echo "== cold full index (fresh data dirs)"
ROGREP_COLD=$(time_cmd "$ROGREP_BIN" sync)
echo "rogrep sync (cold): ${ROGREP_COLD} ms"
CASS_COLD=$(time_cmd "$CASS_BIN" index --full --json)
echo "cass index --full (cold): ${CASS_COLD} ms"
echo

echo "== warm refresh (no changes)"
echo "rogrep sync (warm): $(run_reps 5 -- "$ROGREP_BIN" sync) ms (min median max)"
echo "cass index (warm):  $(run_reps 5 -- "$CASS_BIN" index --json) ms (min median max)"
echo

echo "== index size on disk"
echo "rogrep: $(du -sh "$ROGREP_DATA_DIR" | cut -f1)  ($(du -sh "$ROGREP_DATA_DIR/index" 2>/dev/null | cut -f1) search index, $(du -sh "$ROGREP_DATA_DIR/db" 2>/dev/null | cut -f1) sqlite)"
echo "cass:   $(du -sh "$CASS_DATA_DIR" | cut -f1)  ($(du -sh "$CASS_DATA_DIR/index" 2>/dev/null | cut -f1) lexical index, $(du -sh "$CASS_DATA_DIR"/agent_search.db* 2>/dev/null | awk '{s+=$1} END {print s"?"}' | head -c0; du -ch "$CASS_DATA_DIR"/agent_search.db* 2>/dev/null | tail -1 | cut -f1) sqlite)"
echo

echo "== indexed volume (self-reported)"
"$ROGREP_BIN" ls --limit 1 >/dev/null 2>&1 || true
echo "rogrep conversations: $("$ROGREP_BIN" stats projects 2>/dev/null | awk 'NR>1 {s+=$2} END {print s}')"
echo "cass: $("$CASS_BIN" health --json 2>/dev/null | head -c 400 || true)"
echo

QUERIES=(
  "tantivy"
  "flaky test"
  "swap OOM killer"
  "gh pr create"
  "conversation index rebuild"
  "\"cargo build --release\""
)

echo "== end-to-end search latency, ms (min median max over $REPS reps, --limit 10)"
for q in "${QUERIES[@]}"; do
  r=$(run_reps "$REPS" -- "$ROGREP_BIN" search "$q" --limit 10)
  c=$(run_reps "$REPS" -- "$CASS_BIN" search "$q" --robot --limit 10)
  printf '%-28s rogrep: %-18s cass: %s\n' "\"$q\"" "$r" "$c"
done
echo

echo "== result sanity (both tools must return hits for every query)"
for q in "${QUERIES[@]}"; do
  rc=$("$ROGREP_BIN" search "$q" --limit 50 2>/dev/null | grep -c '^rg_' || true)
  cbytes=$("$CASS_BIN" search "$q" --robot --limit 50 2>/dev/null | wc -c)
  cstatus="non-empty"
  [ "$cbytes" -lt 3 ] && cstatus="EMPTY"
  printf '%-28s rogrep conversations: %-4s cass output: %s (%s bytes)\n' "\"$q\"" "$rc" "$cstatus" "$cbytes"
done
echo
echo "note: hit-count semantics differ between the tools (conversations vs"
echo "messages, different corpus discovery); this phase only checks neither"
echo "engine comes up empty. Quality/ranking is out of scope."
