#!/usr/bin/env bash
# Live Claude spawn-boundary probe (issues #20/#21/#23).
#
# Point CLAUDE_BIN at this file and CLAUDE_CODE_PATH at the real Claude Code
# executable. The Agent SDK spawns this wrapper with the exact runner env and
# argv; after recording key NAMES only, `exec` replaces it with the real Claude
# binary without changing either. No environment value is ever written.

set +x
set -euo pipefail
umask 077

: "${TMPDIR:?TMPDIR must name the private evidence directory}"
: "${CLAUDE_CODE_PATH:?CLAUDE_CODE_PATH must name the real Claude executable}"

evidence_path="${TMPDIR}/claude-runner-env-audit.json"
expected_denied=(
  GH_BIN
  GH_TOKEN
  MATRIX_LIVE_AUDIT
  ADW_LIVE_AUDIT
  MX_AGENT_LIVE_AUDIT
)
observed_denied=()
observed_controls=()

while IFS= read -r key; do
  case "${key}" in
    GH_BIN|GH_TOKEN|MATRIX_*|ADW_*|MX_AGENT_*) observed_denied+=("${key}") ;;
    HOME|PATH|TMPDIR|CLAUDE_BIN|CLAUDE_CODE_PATH|CLAUDE_CODE_ENTRYPOINT|CLAUDE_AGENT_SDK_VERSION)
      observed_controls+=("${key}")
      ;;
  esac
done < <(compgen -e)

model=""
budget=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i += 1)); do
  case "${args[i]}" in
    --model)
      if ((i + 1 < ${#args[@]})); then model="${args[i + 1]}"; fi
      ;;
    --max-budget-usd)
      if ((i + 1 < ${#args[@]})); then budget="${args[i + 1]}"; fi
      ;;
  esac
done

json_string() {
  local value="${1}"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  value="${value//$'\r'/\\r}"
  value="${value//$'\t'/\\t}"
  printf '"%s"' "${value}"
}

json_array() {
  local first=true value
  printf '['
  for value in "$@"; do
    if [[ "${first}" == true ]]; then
      first=false
    else
      printf ', '
    fi
    json_string "${value}"
  done
  printf ']'
}

result=PASS
if ((${#observed_denied[@]} > 0)); then result=FAIL; fi

# Keep the probe itself inside the boundary: this evidence writer uses Bash
# built-ins only, so a failed audit cannot forward denied variables to a helper.
{
  printf '{\n'
  printf '  "schema_version": 1,\n'
  printf '  "boundary": "claude-agent-sdk spawned executable",\n'
  printf '  "value_output_forbidden": true,\n'
  printf '  "expected_parent_poisoned_keys": '
  json_array "${expected_denied[@]}"
  printf ',\n  "observed_denied_key_names": '
  json_array "${observed_denied[@]}"
  printf ',\n  "observed_control_key_names": '
  json_array "${observed_controls[@]}"
  printf ',\n  "requested_model": '
  if [[ -n "${model}" ]]; then json_string "${model}"; else printf 'null'; fi
  printf ',\n  "requested_max_budget_usd": '
  if [[ -n "${budget}" ]]; then json_string "${budget}"; else printf 'null'; fi
  printf ',\n  "result": '
  json_string "${result}"
  printf '\n}\n'
} > "${evidence_path}"

if ((${#observed_denied[@]} > 0)); then
  printf 'claude env audit: FAIL (%s)\n' "$(IFS=,; printf '%s' "${observed_denied[*]}")" >&2
  exit 97
fi

printf 'claude env audit: PASS (denied key names absent; denied values never inspected or printed)\n' >&2
exec "${CLAUDE_CODE_PATH}" "$@"
