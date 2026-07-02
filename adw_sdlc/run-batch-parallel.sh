#!/usr/bin/env bash
# Parallel ADW batch runner — worktree-per-run lanes, dependency-aware phases.
# Supersedes run-batch.sh (sequential) for multi-lane batches. Design notes:
#   * One linked git worktree per issue run (--project-root <wt>): per-run state
#     (agents/<adw-id>/) lives inside the worktree, so lanes never share state,
#     and /agents/ + /target/ are root-anchored gitignores in iroh-room.
#   * --repo is ALWAYS passed: it is what makes `gh pr merge --delete-branch`
#     skip the local-branch checkout that would fail inside a linked worktree.
#   * The kernel rebases onto origin/<base> and re-proves the gates before every
#     merge (git.ts syncWithBase), so cross-lane merges cannot land a branch
#     validated against a stale base. A CONFLICTED rebase aborts the run and is
#     retried FRESH here — --resume would fail identically forever, so the reset
#     drops the local+remote branch and the worktree, then reruns from the new base.
#   * Lanes are sequential dependency chains; phases are barriers. Concurrency is
#     capped (one gh token, one claude.ai subscription, N cold cargo builds) and
#     lane starts are staggered to dodge shared-.git config.lock/ref-lock races.
#   * If gh starts rate-limiting progress comments, add --no-progress to CMD below.
# Usage:  ./run-batch-parallel.sh [--dry-run]
set -uo pipefail

# ── Config ──────────────────────────────────────────────────────────────────
ADW_DIR=/Users/sekou/TAC/pi-gh-issue/adw_sdlc
MAIN_CHECKOUT=/Users/sekou/TAC/iroh-room
WT_ROOT=/Users/sekou/TAC/ir-worktrees
REPO=kortiene/iroh-room
RUNNER=claude
MAX_ATTEMPTS=4            # a base conflict consumes one cheap attempt (resume → rebase-fail → reset)
MAX_CONCURRENT=3
STAGGER_SECS=45
TIMEOUT_SECS=3600
PER_RUN_BUDGET_USD=45
TOTAL_BUDGET_USD=800      # hard stop for the whole batch (median run ≈ $40)

# Dependency plan (verified against issue bodies 2026-07-01). A lane is a
# sequential chain in one worktree slot; lanes in a phase run in parallel;
# phases are barriers gated on the blocking issues below + parity:rate ≤10%.
PHASE_A_LANES=("28 29 30" "31 32 33" "25 45" "10 15")
PHASE_B_LANES=("34")                    # p0/risk-high full-demo test — solo
PHASE_C_LANES=("36 39" "38" "37 40")    # 38 out of lane 5: conflicts with 36's refactor
PHASE_D_LANES=("41")                    # aggregates 37–40 — last
BLOCKING_AFTER_A="29 32 33"             # 34 needs these merged (30 is NOT required)
BLOCKING_AFTER_B="34 25"                # epic #4 gates Phase 2 on 34; #38 (Phase C) needs #25
BLOCKING_AFTER_C="37 38 39 40"          # 41 aggregates all four

EPICS="1 2 3 4"                         # tracking issues — never run, close at end
epic_children () {
  case "$1" in
    1) echo "10 15" ;;
    2) echo "15 25" ;;
    3) echo "28 29 30 31 32 33 34" ;;
    4) echo "34 36 37 38 39 40 41" ;;
  esac
}

DRY_RUN=0
[ "${1:-}" = "--dry-run" ] && DRY_RUN=1

TS="$(date +%Y%m%d-%H%M%S)"
ART="$ADW_DIR/batch-artifacts-$TS"

# ── Helpers ─────────────────────────────────────────────────────────────────
id_for () { printf 'ba%06x' "$1"; }     # stable 8-hex id per issue → resumable
wt_for () { echo "$WT_ROOT/wt-$1"; }
log () { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*"; }
issue_state () { gh issue view "$1" --repo "$REPO" --json state -q .state 2>/dev/null; }

# Robust sum of .summary.total_cost_usd across metric files. Per-file isolation:
# a corrupt/empty/null-cost file counts as the per-run cap so a pricing or
# write gap can never unlock extra spend, and one bad file cannot hide the rest.
sum_metrics () {  # <file...>
  local f v total=0
  for f in "$@"; do
    [ -f "$f" ] || continue
    v=$(jq -r '.summary.total_cost_usd // empty' "$f" 2>/dev/null)
    case $v in ''|*[!0-9.]*) v=$PER_RUN_BUDGET_USD ;; esac
    total=$(awk -v a="$total" -v b="$v" 'BEGIN { printf "%.2f", a + b }')
  done
  echo "$total"
}
# metrics.json is per-PROCESS (each attempt overwrites it), so the budget sums
# the per-attempt archives banked by run_one_issue, never the live mirror.
spent_usd () { sum_metrics "$ART"/agents/*/metrics-attempt-*.json; }
budget_exhausted () {
  awk -v s="$(spent_usd)" -v t="$TOTAL_BUDGET_USD" 'BEGIN { exit !(s >= t) }'
}

collect_artifacts () {  # <wt> <adw_id> — copy run state/metrics out before the worktree dies
  local src="$1/agents/$2"
  [ -d "$src" ] || return 0
  # --exclude keeps the banked per-attempt cost archives safe from --delete.
  rsync -a --delete --exclude='metrics-attempt-*.json' "$src/" "$ART/agents/$2/" 2>/dev/null \
    || cp -R "$src" "$ART/agents/" 2>/dev/null || true
}

ensure_worktree () {  # <wt>
  [ -d "$1" ] && return 0
  git -C "$MAIN_CHECKOUT" fetch origin --quiet 2>/dev/null || true
  git -C "$MAIN_CHECKOUT" worktree add --detach "$1" origin/main >/dev/null 2>&1 && return 0
  # Heal 'missing but already registered worktree': the dir vanished without
  # deregistration; remove --force on a missing path exits 0, then retry once.
  remove_worktree "$1"
  git -C "$MAIN_CHECKOUT" worktree add --detach "$1" origin/main >/dev/null 2>&1
}
remove_worktree () { git -C "$MAIN_CHECKOUT" worktree remove --force "$1" >/dev/null 2>&1 || true; }

branch_of () {  # <wt> <adw_id> <issue> — from state.json, else deterministic pattern
  local b
  b=$(jq -r '.branch_name // empty' "$1/agents/$2/state.json" 2>/dev/null)
  if [ -z "$b" ]; then
    b=$(git -C "$MAIN_CHECKOUT" branch --list "*/$3-$2-*" --format='%(refname:short)' 2>/dev/null | head -1)
  fi
  echo "$b"
}
delete_branch () {  # <branch> — local + remote, best-effort; caller guards the merged case
  [ -n "$1" ] || return 0
  git -C "$MAIN_CHECKOUT" branch -D "$1" >/dev/null 2>&1 || true
  git -C "$MAIN_CHECKOUT" push origin --delete "$1" >/dev/null 2>&1 || true
}
pr_merged_for_branch () {  # <branch> — merged AND its head SHA is our local tip
  # Branch names are deterministic across batches (issue + adw-id + slug), so
  # name-only matching would count a PREVIOUS batch's merged PR as this run's.
  [ -n "$1" ] || return 1
  local tip
  tip="$(git -C "$MAIN_CHECKOUT" rev-parse --verify --quiet "refs/heads/$1")" || return 1
  gh pr list --repo "$REPO" --head "$1" --state merged --json headRefOid \
    --jq '.[].headRefOid' 2>/dev/null | grep -qx "$tip"
}

# ── Per-issue runner ────────────────────────────────────────────────────────
# 0 done · 1 failed (attempts exhausted) · 2 skipped (already closed) · 3 budget stop
run_one_issue () {  # <issue> <lane>
  local n="$1" lane="$2" adw_id wt attempt rc alog branch
  local resume=()
  # Per-issue cap, raised ONCE after a budget trip: the accumulated cost lives
  # in state.json, so a same-cap resume of an over-budget run fails fast in the
  # kernel — the recovery is a raised cap, not a blind retry. A second trip at
  # the raised cap means the issue is genuinely oversized: fail it.
  local cap="$PER_RUN_BUDGET_USD" cap_raised=0
  adw_id="$(id_for "$n")"; wt="$(wt_for "$n")"

  if [ "$(issue_state "$n")" = "CLOSED" ]; then
    log "lane $lane: #$n already CLOSED — skipping"
    echo skipped > "$ART/status/$n"; return 2
  fi

  for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
    if budget_exhausted; then
      log "lane $lane: total budget \$$TOTAL_BUDGET_USD reached — not starting #$n"
      echo budget-stop > "$ART/status/$n"; return 3
    fi
    if ! ensure_worktree "$wt"; then
      log "lane $lane: worktree add failed for #$n (attempt $attempt)"; sleep 15; continue
    fi

    resume=()
    [ -f "$wt/agents/$adw_id/state.json" ] && resume=(--resume)
    if [ "$attempt" -gt 1 ] && [ "${#resume[@]}" -gt 0 ]; then
      # fmt-nit salvage from run-batch.sh — belt-and-braces over the kernel's gate-heal
      ( cd "$wt" && cargo fmt --all >/dev/null 2>&1 || true )
    fi

    # metrics.json is per-process: clear it before the attempt so any file
    # present afterwards is definitively THIS attempt's spend (resume never
    # reads it), then bank it as an append-only per-attempt archive that the
    # collect_artifacts mirror cannot delete. spent_usd sums the archives.
    rm -f "$wt/agents/$adw_id/metrics.json"

    alog="$ART/logs/issue-$n-attempt-$attempt.log"
    log "lane $lane: ADW #$n attempt $attempt/$MAX_ATTEMPTS id=$adw_id ${resume[*]:-fresh} wt=$wt"
    ( cd "$ADW_DIR" && npm run issue -- "$n" --runner "$RUNNER" \
        --project-root "$wt" --repo "$REPO" --adw-id "$adw_id" \
        --timeout "$TIMEOUT_SECS" --max-budget-usd "$cap" \
        ${resume[@]+"${resume[@]}"} -y ) >"$alog" 2>&1
    rc=$?
    collect_artifacts "$wt" "$adw_id"
    if [ -f "$wt/agents/$adw_id/metrics.json" ]; then
      mkdir -p "$ART/agents/$adw_id"
      cp "$wt/agents/$adw_id/metrics.json" "$ART/agents/$adw_id/metrics-attempt-$attempt.json" 2>/dev/null || true
    fi

    # GitHub's issue auto-close can lag the squash-merge by a few seconds; a
    # "still OPEN after merge" verify failure deserves one settled re-read
    # before being treated as a real failure (observed live).
    if [ "$rc" -ne 0 ] && grep -Eq "still OPEN after merge|is OPEN despite a recorded merge" "$alog" 2>/dev/null; then
      sleep 20  # both kernel wordings: fresh-finalize verify and the resume guard
    fi
    if [ "$rc" -eq 0 ] || [ "$(issue_state "$n")" = "CLOSED" ]; then
      [ "$rc" -eq 0 ] || log "lane $lane: #$n reports CLOSED despite rc=$rc — counting as done"
      log "lane $lane: #$n DONE"
      branch="$(branch_of "$wt" "$adw_id" "$n")"  # capture before the state dies with the worktree
      remove_worktree "$wt"                        # branch -D fails while checked out here
      delete_branch "$branch"                      # remote is already gone post-merge
      echo "done" > "$ART/status/$n"; return 0
    fi

    log "lane $lane: #$n attempt $attempt failed (rc=$rc): $(tail -c 400 "$alog" | tr '\n' ' ' | tail -c 200)"
    branch="$(branch_of "$wt" "$adw_id" "$n")"

    if pr_merged_for_branch "$branch"; then
      # Remote merge landed but the run died after (e.g. at verify): close the
      # loop ourselves rather than re-implementing work that is already on main.
      log "lane $lane: #$n PR already MERGED — closing the issue and counting as done"
      gh issue close "$n" --repo "$REPO" \
        --comment "Landed via merged PR for branch \`$branch\` (ADW batch $TS)." >/dev/null 2>&1 || true
      remove_worktree "$wt"
      delete_branch "$branch"
      echo "done" > "$ART/status/$n"; return 0
    fi

    # A budget trip is not a retryable flake: raise the cap once, else fail.
    if grep -qF "the budget cap" "$alog"; then
      if [ "$cap_raised" = 0 ]; then
        cap=$((PER_RUN_BUDGET_USD + 30)); cap_raised=1
        log "lane $lane: #$n tripped the \$$PER_RUN_BUDGET_USD budget cap — resuming with --max-budget-usd $cap"
      else
        log "lane $lane: #$n tripped the RAISED cap (\$$cap) — genuinely oversized, failing"
        echo "failed" > "$ART/status/$n"
        return 1
      fi
    fi

    # Fresh-reset ONLY on the kernel's exact rebase-conflict line. Transient
    # merge/push/fetch failures deliberately fall through to --resume (cheap);
    # a genuine conflict resurfaces on resume as this exact error and resets then.
    # NEVER on the final attempt: with no retry left, the reset would only
    # destroy the state a manual salvage needs (cherry-pick + resolve + resume).
    if grep -qF "rebase onto origin/" "$alog" && grep -qF "a fresh run from the moved base is required" "$alog"; then
      if [ "$attempt" -lt "$MAX_ATTEMPTS" ]; then
        log "lane $lane: #$n conflicts with the moved base — fresh reset (drop branch + worktree)"
        remove_worktree "$wt"
        delete_branch "$branch"
        # next attempt: brand-new worktree, no state.json → fresh run from the new base
      else
        log "lane $lane: #$n conflicts with the moved base on the FINAL attempt — keeping branch/worktree for manual salvage"
      fi
    fi
    sleep 5
  done

  echo failed > "$ART/status/$n"
  return 1
}

run_lane () {  # <lane_name> <issue...>
  local lane="$1"; shift
  local n
  for n in "$@"; do
    run_one_issue "$n" "$lane"
    case "$?" in
      0|2) ;;  # done/skipped → next link of the chain
      3) log "lane $lane: stopping on batch budget"; return 3 ;;
      *) log "lane $lane: #$n FAILED after $MAX_ATTEMPTS attempts — stopping this lane (downstream depends on it)"
         return 1 ;;
    esac
  done
  return 0
}

# ── Phase orchestration ─────────────────────────────────────────────────────
active_jobs () { jobs -rp | wc -l | tr -d ' '; }

run_phase () {  # <phase_name> <lane...>
  local phase="$1"; shift
  log "═══ Phase $phase — $# lane(s): $(printf '[%s] ' "$@")"
  local pids=() lane pid i=0
  for lane in "$@"; do
    while [ "$(active_jobs)" -ge "$MAX_CONCURRENT" ]; do sleep 10; done
    [ "$i" -gt 0 ] && sleep "$STAGGER_SECS"
    i=$((i + 1))
    # The lane string is a deliberate word-split issue list.
    # shellcheck disable=SC2086
    run_lane "$phase$i" $lane >"$ART/logs/lane-$phase$i.log" 2>&1 &
    pid=$!; pids+=("$pid")
    log "phase $phase: launched lane $phase$i ($lane) pid=$pid → $ART/logs/lane-$phase$i.log"
  done
  for pid in ${pids[@]+"${pids[@]}"}; do wait "$pid" || true; done
}

report_progress () {  # <phase_name>
  local phase="$1" done_l="" skip_l="" fail_l="" other_l="" f n st
  for f in "$ART/status/"*; do
    [ -e "$f" ] || continue
    n="$(basename "$f")"; st="$(cat "$f")"
    case "$st" in
      done)    done_l="$done_l #$n" ;;
      skipped) skip_l="$skip_l #$n" ;;
      failed)  fail_l="$fail_l #$n" ;;
      *)       other_l="$other_l #$n($st)" ;;
    esac
  done
  log "── after phase $phase — done:${done_l:- none} · skipped:${skip_l:- none} · failed:${fail_l:- none} · other:${other_l:- none} · spent: \$$(spent_usd)"
}

parity_check () {
  ls "$ART/agents"/*/metrics.json >/dev/null 2>&1 || return 0
  ( cd "$ADW_DIR" && npm run parity:rate -- --max-native-rate 10 "$ART/agents/" ) >>"$ART/logs/parity.log" 2>&1
}

phase_barrier () {  # <phase_name> <blocking issue...>
  local phase="$1"; shift
  report_progress "$phase"
  if ! parity_check; then
    log "parity gate (>10% hard-fail) tripped after phase $phase — stopping batch; see $ART/logs/parity.log"
    finish 1
  fi
  local n st
  for n in "$@"; do
    st="$(cat "$ART/status/$n" 2>/dev/null || echo missing)"
    if [ "$st" != "done" ] && [ "$st" != "skipped" ]; then
      log "phase $phase barrier: blocking issue #$n is '$st' — downstream depends on it; stopping batch"
      finish 1
    fi
  done
}

close_epics () {
  local e c all_closed
  for e in $EPICS; do
    [ "$(issue_state "$e")" = "OPEN" ] || continue
    all_closed=1
    for c in $(epic_children "$e"); do
      [ "$(issue_state "$c")" = "CLOSED" ] || { all_closed=0; break; }
    done
    if [ "$all_closed" = 1 ]; then
      log "closing epic #$e — all children CLOSED"
      gh issue close "$e" --repo "$REPO" \
        --comment "All child work items are complete (ADW batch $TS)." >/dev/null 2>&1 || true
    fi
  done
}

final_summary () {
  echo; echo "===== BATCH SUMMARY ====="
  local f st m cost dur
  for f in $(ls "$ART/status" 2>/dev/null | sort -n); do
    st="$(cat "$ART/status/$f")"
    m="$ART/agents/$(id_for "$f")/metrics.json"
    cost=$(sum_metrics "$ART/agents/$(id_for "$f")"/metrics-attempt-*.json)  # cumulative across attempts
    if [ -f "$m" ]; then
      dur=$(jq -r '((.summary.total_duration_ms // 0) / 60000) | round' "$m" 2>/dev/null || echo '?')
      echo "  #$f: $st (\$$cost, last attempt ${dur}min)"
    else
      echo "  #$f: $st (\$$cost)"
    fi
  done
  echo "  total spent : \$$(spent_usd) of \$$TOTAL_BUDGET_USD"
  echo "  artifacts   : $ART"
  echo "  worktrees   :"; git -C "$MAIN_CHECKOUT" worktree list | sed 's/^/    /'
}

finish () { final_summary; exit "$1"; }

# ── Dry run ─────────────────────────────────────────────────────────────────
dry_run () {
  local runnable=0
  print_phase () {
    local name="$1"; shift
    local i=0 lane n st
    echo; echo "Phase $name:"
    for lane in "$@"; do
      i=$((i + 1)); printf '  lane %s%s:' "$name" "$i"
      for n in $lane; do
        st="$(issue_state "$n")"
        if [ "$st" = "CLOSED" ]; then
          printf ' #%s(skip:closed)' "$n"
        else
          printf ' #%s' "$n"; runnable=$((runnable + 1))
        fi
      done
      echo
      for n in $lane; do
        echo "      #$n → adw-id=$(id_for "$n")  wt=$(wt_for "$n")"
      done
    done
  }
  echo "ADW parallel batch — DRY RUN (no side effects)"
  echo "repo=$REPO  runner=$RUNNER  concurrency=$MAX_CONCURRENT lanes  stagger=${STAGGER_SECS}s"
  echo "per-run: --timeout $TIMEOUT_SECS --max-budget-usd $PER_RUN_BUDGET_USD   batch hard stop: \$$TOTAL_BUDGET_USD"
  print_phase A "${PHASE_A_LANES[@]}"
  print_phase B "${PHASE_B_LANES[@]}"
  print_phase C "${PHASE_C_LANES[@]}"
  print_phase D "${PHASE_D_LANES[@]}"
  echo
  echo "command : npm run issue -- <N> --runner $RUNNER --project-root <wt> --repo $REPO --adw-id <id> [--resume] --timeout $TIMEOUT_SECS --max-budget-usd $PER_RUN_BUDGET_USD -y"
  echo "barriers: A→B needs [$BLOCKING_AFTER_A] · B→C needs [$BLOCKING_AFTER_B] · C→D needs [$BLOCKING_AFTER_C] · parity:rate ≤10% at each"
  echo "estimate: $runnable runnable issues ≈ \$$((runnable * 40)) (range \$$((runnable * 18))–\$$((runnable * 51)), median \$40, 1–3h each)"
}

# ── Main ────────────────────────────────────────────────────────────────────
main () {
  command -v jq >/dev/null || { echo "jq is required"; exit 1; }
  if [ "$DRY_RUN" = 1 ]; then dry_run; exit 0; fi

  mkdir -p "$ART/logs" "$ART/status" "$ART/agents" "$WT_ROOT"
  exec > >(tee "$ART/logs/batch.log") 2>&1
  unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN  # depleted PAYG keys shadow the claude.ai login
  # set -m gives each lane its own process group so the trap can kill the
  # npm/tsx/claude GRANDCHILDREN too — without it an interrupt leaves up to
  # MAX_CONCURRENT agent runs burning money and merging PRs unattended. With
  # job control on, this trap is the ONLY cleanup path for Ctrl-C: keep it.
  set -m
  # shellcheck disable=SC2154
  trap 'log "interrupted — killing lane process groups"; for p in $(jobs -p); do kill -TERM -- "-$p" 2>/dev/null; done; exit 130' INT TERM

  gh auth status >/dev/null 2>&1 || { log "gh is not authenticated"; exit 1; }
  git -C "$MAIN_CHECKOUT" fetch origin --quiet || { log "cannot fetch in $MAIN_CHECKOUT"; exit 1; }
  git -C "$MAIN_CHECKOUT" worktree prune >/dev/null 2>&1 || true
  log "batch $TS starting — artifacts in $ART"

  run_phase A "${PHASE_A_LANES[@]}"
  phase_barrier A $BLOCKING_AFTER_A
  run_phase B "${PHASE_B_LANES[@]}"
  phase_barrier B $BLOCKING_AFTER_B
  run_phase C "${PHASE_C_LANES[@]}"
  phase_barrier C $BLOCKING_AFTER_C
  run_phase D "${PHASE_D_LANES[@]}"
  report_progress D
  parity_check || log "WARNING: parity gate failed on the final artifact set — see $ART/logs/parity.log"

  close_epics
  finish 0
}
main "$@"
