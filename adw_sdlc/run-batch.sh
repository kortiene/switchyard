#!/usr/bin/env bash
# Robust ADW batch runner — hardened from the #8→#27 run's four stalls:
#   #9  fmt nit failed the gate & killed a $48 run  → cargo fmt before each retry
#   #7  transient API 500 in review phase           → resume-retry re-enters at review
#   #43 auth outage ("Not logged in") at implement  → resume-retry + unset dead key
#   #24 CI checked before it finished → false "not green" → resume-retry re-runs merge
# Plus: skip already-CLOSED issues (idempotent), timestamped log, pass/fail summary.
set -uo pipefail

ADW_DIR=/Users/sekou/TAC/pi-gh-issue/adw_sdlc
PROJECT_ROOT=/Users/sekou/TAC/iroh-room
REPO=kortiene/iroh-room
RUNNER=claude
MAX_ATTEMPTS=3
# Next 10 after #27 — Phase 1B feature series (dependency order), then Phase 0 loose ends.
# #34 (IR-0209 full demo) MUST stay last of the 1B group: it depends on 28–33.
ISSUES=(28 29 30 31 32 33 34 45 10 15)

cd "$ADW_DIR"
LOG="$ADW_DIR/adw-batch-$(date +%Y%m%d-%H%M%S).log"
exec > >(tee "$LOG") 2>&1          # tee in the parent shell so summary arrays survive
unset ANTHROPIC_API_KEY            # exhausted key otherwise wins over the claude.ai login

id_for () { printf 'ba%06x' "$1"; }        # stable 8-hex id per issue → resumable

declare -a DONE SKIPPED FAILED
for n in "${ISSUES[@]}"; do
  if [ "$(gh issue view "$n" --repo "$REPO" --json state -q .state 2>/dev/null)" = CLOSED ]; then
    echo ">>> #$n already CLOSED — skipping"; SKIPPED+=("$n"); continue
  fi
  adw_id="$(id_for "$n")"; ok=0
  for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
    resume=(); [ -f "$PROJECT_ROOT/agents/$adw_id/state.json" ] && resume=(--resume)
    if [ "$attempt" -gt 1 ]; then                    # salvage the fmt-nit class, then resume
      echo ">>> #$n retry $attempt: cargo fmt --all + resume"
      ( cd "$PROJECT_ROOT" && cargo fmt --all >/dev/null 2>&1 || true )
      resume=(--resume)
    fi
    echo ">>> ADW #$n  attempt $attempt/$MAX_ATTEMPTS  id=$adw_id ${resume[*]:-fresh}"
    if npm run issue -- "$n" --runner "$RUNNER" \
         --project-root "$PROJECT_ROOT" --repo "$REPO" \
         --adw-id "$adw_id" "${resume[@]}" -y; then ok=1; break; fi
    echo ">>> #$n attempt $attempt failed"
  done
  if [ "$ok" = 1 ]; then DONE+=("$n")
  else FAILED+=("$n"); echo "FAILED at #$n after $MAX_ATTEMPTS attempts — stopping (downstream depends on it)"; break; fi
done

echo "===== SUMMARY ====="
echo "done    : ${DONE[*]:-none}"
echo "skipped : ${SKIPPED[*]:-none}"
echo "failed  : ${FAILED[*]:-none}"
echo "log     : $LOG"
