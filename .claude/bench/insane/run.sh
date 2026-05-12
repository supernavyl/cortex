#!/usr/bin/env bash
# Insane bench runner — cortex vs Claude Code on hard Rust tasks.
#
# Usage:
#   .claude/bench/insane/run.sh smoke              # 3 tasks × 2 systems
#   .claude/bench/insane/run.sh full               # 13 tasks × 2 systems
#   .claude/bench/insane/run.sh task <name>        # 1 task × 2 systems
#
# Output:
#   /tmp/insane-bench-<timestamp>/<system>/<task>/...
#   /tmp/insane-bench-<timestamp>/results.tsv

set -uo pipefail

REPO_ROOT="/home/supernovyl/projects/cortex"
BENCH_TIER="${BENCH_TIER:-project}"   # 'algo' or 'project'
case "$BENCH_TIER" in
  algo)    TASKS_DIR="$REPO_ROOT/.claude/bench/insane/tasks" ;;
  project) TASKS_DIR="$REPO_ROOT/.claude/bench/insane/projects" ;;
  *) echo "BENCH_TIER must be 'algo' or 'project'"; exit 2 ;;
esac
CORTEX_MODEL="${CORTEX_MODEL:-kimi-k2.6:cloud}"
TIMEOUT_SECS="${TIMEOUT_SECS:-900}"   # 15 min per project task
FAKE_PASS_THRESHOLD="${FAKE_PASS_THRESHOLD:-30}"  # rust_lines below = NOPROD

if [[ "$BENCH_TIER" == "algo" ]]; then
  SMOKE_TASKS=(rope bloom_filter lru_threadsafe)
  FULL_TASKS=(rope persistent_btree wal_log bloom_filter skip_list_atomic dpll_sat \
              deflate_decoder hyperloglog lru_threadsafe lisp_interp regex_nfa \
              radix_sort_generic paxos_basic)
else
  SMOKE_TASKS=(kv_http jq_clone rate_limiter)
  FULL_TASKS=(kv_http cron_scheduler markdown_ssg jq_clone proto_codec git_tiny \
              sql_parser actor_fw rate_limiter ws_echo bft_sim iso9660_reader \
              cache_proxy)
fi

SYSTEMS=(cortex claude)
SYSTEMS_FILTER="${SYSTEMS_FILTER:-}"  # comma-separated subset, e.g. "cortex" to skip claude
if [[ -n "$SYSTEMS_FILTER" ]]; then
  IFS=',' read -ra SYSTEMS <<< "$SYSTEMS_FILTER"
fi

mode="${1:-smoke}"
single_task="${2:-}"

case "$mode" in
  smoke) tasks=("${SMOKE_TASKS[@]}") ;;
  full)  tasks=("${FULL_TASKS[@]}") ;;
  task)  tasks=("$single_task") ;;
  *) echo "usage: $0 {smoke|full|task <name>}"; exit 2 ;;
esac

stamp=$(date +%Y%m%d-%H%M%S)
BENCH_DIR="/tmp/insane-bench-$stamp"
mkdir -p "$BENCH_DIR"
RESULTS_TSV="$BENCH_DIR/results.tsv"

printf 'system\ttask\tverdict\trun_exit\tcheck_pass\ttest_pass\tlatency_s\trust_lines\n' > "$RESULTS_TSV"

echo "============================================================"
echo "  INSANE BENCH"
echo "============================================================"
echo "  Tier:    $BENCH_TIER"
echo "  Mode:    $mode"
echo "  Tasks:   ${tasks[*]}"
echo "  Systems: ${SYSTEMS[*]}"
echo "  Cortex:  $CORTEX_MODEL"
echo "  Claude:  $(claude --version 2>&1 | head -1)"
echo "  Output:  $BENCH_DIR"
echo "  Timeout: ${TIMEOUT_SECS}s per run"
echo "  FakePass threshold: rust_lines < $FAKE_PASS_THRESHOLD => NOPROD"
echo "============================================================"

seed_workspace() {
  local ws="$1"
  local name="$2"
  mkdir -p "$ws/src"
  cat > "$ws/Cargo.toml" <<EOF
[package]
name = "$name"
version = "0.0.1"
edition = "2024"

[dependencies]
EOF
  echo "// scaffold" > "$ws/src/lib.rs"
}

count_rust_lines() {
  local ws="$1"
  find "$ws" -name "*.rs" -not -path "*/target/*" -exec cat {} + 2>/dev/null | wc -l
}

ensure_cortex_daemon() {
  local sock="$HOME/.config/cortex/daemon.sock"
  # Daemon is healthy iff a cortex-daemon process exists AND the socket is present.
  if pgrep -f "target/debug/cortex-daemon" >/dev/null 2>&1 && [[ -S "$sock" ]]; then
    return 0
  fi
  # Otherwise: kill any lingering process, remove stale socket, start fresh.
  pkill -f "target/debug/cortex-daemon" 2>/dev/null || true
  rm -f "$sock"
  sleep 0.5
  ( cd "$REPO_ROOT" && nohup "$REPO_ROOT/target/debug/cortex-daemon" \
      > /tmp/cortex-daemon-bench.log 2>&1 & )
  # Wait up to 10s for the socket to appear
  for _ in $(seq 1 20); do
    if pgrep -f "target/debug/cortex-daemon" >/dev/null 2>&1 && [[ -S "$sock" ]]; then
      return 0
    fi
    sleep 0.5
  done
  echo "ERROR: cortex-daemon failed to start within 10s" >&2
  return 1
}

run_cortex() {
  local ws="$1"
  local prompt_file="$2"
  local log="$ws/_run.log"
  local prompt
  prompt=$(cat "$prompt_file")
  ensure_cortex_daemon || return 99
  ( cd "$ws" && timeout --kill-after=30s "$TIMEOUT_SECS" \
      "$REPO_ROOT/target/debug/cortex-cli" apply "$prompt" --model "$CORTEX_MODEL" \
      > "$log" 2>&1 )
  return $?
}

run_claude() {
  local ws="$1"
  local prompt_file="$2"
  local log="$ws/_run.log"
  ( cd "$ws" && timeout --kill-after=30s "$TIMEOUT_SECS" bash -c \
      "cat '$prompt_file' | claude --print --dangerously-skip-permissions --add-dir '$ws'" \
      > "$log" 2>&1 )
  return $?
}

run_one() {
  local system="$1"
  local task="$2"
  local prompt_file="$TASKS_DIR/$task.md"

  if [[ ! -f "$prompt_file" ]]; then
    echo "  [SKIP] no prompt at $prompt_file"
    printf '%s\t%s\tNOPROMPT\t-\t-\t-\t-\t-\n' "$system" "$task" >> "$RESULTS_TSV"
    return
  fi

  local ws="$BENCH_DIR/$system/$task"
  seed_workspace "$ws" "$task"

  echo
  echo "── $system / $task ──────────────────────────────────────"
  local t0=$(date +%s)

  local run_exit=0
  case "$system" in
    cortex) run_cortex "$ws" "$prompt_file" ;;
    claude) run_claude "$ws" "$prompt_file" ;;
  esac
  run_exit=$?

  local t1=$(date +%s)
  local latency=$(( t1 - t0 ))

  ( cd "$ws" && cargo check --offline --quiet > "$ws/_check.log" 2>&1 )
  local check_pass=$?

  ( cd "$ws" && timeout 120 cargo test --offline --quiet > "$ws/_test.log" 2>&1 )
  local test_pass=$?

  local rust_lines
  rust_lines=$(count_rust_lines "$ws")

  # Verdict: NOPROD if no real code produced; FAIL if produced but doesn't compile/test;
  #         PARTIAL if compiles but tests fail; PASS only if everything is green.
  local verdict
  if [[ $rust_lines -lt $FAKE_PASS_THRESHOLD ]]; then
    verdict="NOPROD"
  elif [[ $check_pass -ne 0 ]]; then
    verdict="FAIL"
  elif [[ $test_pass -ne 0 ]]; then
    verdict="PARTIAL"
  else
    verdict="PASS"
  fi

  printf '%s\t%s\t%s\t%d\t%d\t%d\t%d\t%d\n' \
    "$system" "$task" "$verdict" "$run_exit" "$check_pass" "$test_pass" "$latency" "$rust_lines" \
    >> "$RESULTS_TSV"

  local check_mark="FAIL"; [[ $check_pass -eq 0 ]] && check_mark="PASS"
  local test_mark="FAIL";  [[ $test_pass  -eq 0 ]] && test_mark="PASS"
  echo "  verdict=$verdict  exit=$run_exit  cargo check=$check_mark  cargo test=$test_mark  latency=${latency}s  rust_lines=$rust_lines"
}

for task in "${tasks[@]}"; do
  for system in "${SYSTEMS[@]}"; do
    run_one "$system" "$task"
  done
done

echo
echo "============================================================"
echo "  RESULTS — $BENCH_DIR/results.tsv"
echo "============================================================"
column -t -s$'\t' "$RESULTS_TSV"
echo "============================================================"
