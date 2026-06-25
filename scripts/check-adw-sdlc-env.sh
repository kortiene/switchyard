#!/usr/bin/env bash
#
# check-adw-sdlc-env.sh — fail-closed static lint for the adw_sdlc secret boundary.
#
# This is the grep half of the env-isolation gate described in adw_sdlc/PLAN.md
# (Section 10) and named in src/env.ts: "Runner modules must never spread
# process.env (enforced by scripts/check-adw-sdlc-env.sh and the env-isolation
# tests)." The unit tests assert the runtime behavior of safeSubprocessEnv();
# this script statically forbids the source-level patterns that would silently
# defeat that allowlist before the env object is ever built.
#
# Checks (all fail-closed; any hit exits non-zero):
#   1. No source file spreads `...process.env` — the deny-by-default allowlist
#      in env.ts is the ONLY sanctioned way to build a runner-child env, so a
#      spread would re-leak GH_TOKEN / MATRIX_* / ADW_* / MX_AGENT_* / future secrets
#      (PLAN.md:142). Note: a plain `env: process.env` (orchestrator-owned
#      git/gh subprocesses, never a runner child) is NOT a spread and is allowed.
#   2. No banned @opencode-ai/sdk factory CALLS — createOpencodeServer /
#      createOpencode / createOpencodeTui spread the parent env onto their child
#      (verified on dist/v2/server.js); only createOpencodeClient is permitted
#      (PLAN.md:904).
#   3. The opencode adapter imports the package ONLY via the
#      `@opencode-ai/sdk/v2/client` subpath — any other import/require subpath of
#      @opencode-ai/sdk is banned (PLAN.md:904 "subpath patterns").
#
# Usage: bash scripts/check-adw-sdlc-env.sh   (package.json: `npm run lint:env`)

set -euo pipefail

# Resolve src/ relative to this script so the gate is CWD-independent.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="${SCRIPT_DIR}/../adw_sdlc/src"

if [[ ! -d "${SRC_DIR}" ]]; then
  echo "check-adw-sdlc-env: cannot find source dir at ${SRC_DIR}" >&2
  exit 2
fi

fail=0
report() {
  # $1 = human-readable violation description; remaining = matching lines
  fail=1
  echo "FAIL: $1" >&2
  shift
  while IFS= read -r line; do
    [[ -n "${line}" ]] && echo "    ${line}" >&2
  done <<< "${1:-}"
}

# --- Check 1: no `...process.env` spread anywhere in src ----------------------
# Permit whitespace around the member access so `{ ... process . env }` cannot
# dodge the grep while still allowing ordinary `env: process.env` uses.
spread_hits="$(grep -rnE '\.\.\.[[:space:]]*process[[:space:]]*\.[[:space:]]*env' "${SRC_DIR}" || true)"
if [[ -n "${spread_hits}" ]]; then
  report "a source file spreads ...process.env (use safeSubprocessEnv allowlist instead)" "${spread_hits}"
fi

# --- Check 2: no banned opencode factory calls (createOpencodeClient is OK) ---
# Match call syntax (trailing `(`) so prose/comment mentions in backticks don't
# trip the gate. `createOpencode(` matches createOpencode( and createOpencodeTui(
# /createOpencodeServer( but never createOpencodeClient( (a letter follows).
factory_hits="$(grep -rnE 'createOpencode(Server|Tui)?[[:space:]]*\(' "${SRC_DIR}" || true)"
if [[ -n "${factory_hits}" ]]; then
  report "banned @opencode-ai/sdk factory call (only createOpencodeClient is allowed; the others spread parent env)" "${factory_hits}"
fi

# --- Check 3: opencode imports only via the /v2/client subpath ----------------
# Consider import/export/require statements, including side-effect static
# imports (`import '@opencode-ai/sdk'`) and dynamic imports with optional
# whitespace (`import ('@opencode-ai/sdk')`). The registry's `sdkPackage:` string
# and doc comments are not flagged because they are not import-like syntax.
static_import_hits="$({
  grep -rnE "(^|[;[:space:]])(import|export)([[:space:]][^'\"]*)?['\"]@opencode-ai/sdk[^'\"]*['\"]" "${SRC_DIR}" || true
})"
dynamic_import_hits="$({
  grep -rnE "(import|require)[[:space:]]*\([[:space:]]*['\"]@opencode-ai/sdk[^'\"]*['\"]" "${SRC_DIR}" || true
})"
import_hits="$(printf '%s\n%s\n' "${static_import_hits}" "${dynamic_import_hits}" | awk 'NF' | sort -u)"
if [[ -n "${import_hits}" ]]; then
  bad_imports="$(echo "${import_hits}" | grep -vE "@opencode-ai/sdk/v2/client['\"]" || true)"
  if [[ -n "${bad_imports}" ]]; then
    report "@opencode-ai/sdk imported via a non-/v2/client subpath (only @opencode-ai/sdk/v2/client is allowed)" "${bad_imports}"
  fi
fi

if [[ "${fail}" -ne 0 ]]; then
  echo "" >&2
  echo "check-adw-sdlc-env: secret-boundary lint FAILED (see violations above)." >&2
  exit 1
fi

echo "check-adw-sdlc-env: OK — no ...process.env spread, no banned opencode factory call, opencode imports /v2/client only."
