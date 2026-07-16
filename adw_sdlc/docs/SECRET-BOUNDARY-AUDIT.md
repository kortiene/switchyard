# Secret-boundary audit (preflight + real Claude spawn, no secret printing)

How to assert, on a **real spawned runner environment**, that the denied
secrets — `GH_TOKEN` / `MATRIX_*` / `ADW_*` / legacy `MX_AGENT_*` — are absent,
**without ever printing a secret value** (key NAMES and pass/fail booleans
only).

- **Boundary audited:** [`src/env.ts`](../src/env.ts) — `ENV_DENY_PREFIXES`,
  `safeSubprocessEnv()`.
- **Deterministic preflight:**
  [`test/secret-boundary-audit.test.ts`](../test/secret-boundary-audit.test.ts) (runs inside
  `npm run verify`; uses a trivial Node child, not Claude).
- **Real-runner probe:**
  [`tools/claude-env-audit-wrapper.sh`](../tools/claude-env-audit-wrapper.sh) (the Claude Agent SDK
  spawns it, it audits key names, then `exec`s the real Claude executable).
- **Advances:** [`MVP-READINESS.md`](../MVP-READINESS.md) §1 "operational basics"
  — moves the secret boundary from *statically linted* + *mocked-in-process* to
  *observed on a real spawned child*. Live-run batch row 7
  ([`docs/LIVE-RUN-BATCH.md`](./LIVE-RUN-BATCH.md)).

## 1. Why a fourth layer — four views of one boundary

| Layer | Where | What it proves | Limitation this audit addresses |
| --- | --- | --- | --- |
| **Static lint** | [`scripts/check-adw-sdlc-env.sh`](../../scripts/check-adw-sdlc-env.sh) (`npm run lint:env`) | No source file spreads `...process.env`; opencode factory/import rules | Reads *source text*, not a built env |
| **Mocked unit** | [`test/env.test.ts`](../test/env.test.ts) | `safeSubprocessEnv()` returns an object with no denied keys | Inspects the in-process **object**, never a spawned child |
| **Spawn preflight** | [`test/secret-boundary-audit.test.ts`](../test/secret-boundary-audit.test.ts) + §5 | A trivial Node child spawned with that env observes no denied key | Does not enter the Claude SDK or runner |
| **Real Claude spawn** | [`tools/claude-env-audit-wrapper.sh`](../tools/claude-env-audit-wrapper.sh) + §6 | The executable actually spawned by the Claude Agent SDK sees no denied key, then becomes real Claude via `exec` | Requires credentials and a live, money-spending run |

The deterministic preflight distinguishes itself from `env.test.ts` by crossing a generic
`spawnSync(bin, args, { env })` boundary. It can catch an accidental parent-env merge, but it is
still only `node -e`; by itself it is **not** evidence about a real runner. Section 6 closes that
gap at the exact executable boundary used by the Claude Agent SDK. The wrapper sees the SDK-built
child environment, fails closed if a denied key is present, and otherwise replaces itself with real
Claude without changing argv or env. Any tool process inheriting Claude's environment therefore
cannot regain one of the parent keys that was absent at this boundary.

## 2. The no-secret-printing rule (load-bearing)

This is structural, not just a promise:

1. The audit reports **key NAMES and pass/fail booleans only** — never an
   environment **value**.
2. The deterministic check seeds the *source* env with **sentinels**, not real
   secrets: every denied key in the fixture holds the value
   `SENTINEL-DENIED-MUST-NOT-APPEAR`. So even a hypothetical bug that printed a
   value could only ever leak a sentinel, never a credential.
3. Both spawn probes emit the **names** of any denied keys they can see (expected: none). Neither
   echoes `process.env` wholesale or reads an environment value for reporting.
4. The operator preflight (real `process.env` as source, §5) still prints only the *names* of any
   leaked denied keys, so running it on a machine where a real `GH_TOKEN` / `MATRIX_*` is set cannot
   disclose the value even on failure.
5. The real-runner wrapper (§6) writes a mode-`0600` JSON result under its private `TMPDIR`. Its
   report contains expected/observed **key names**, requested model/budget arguments, and PASS/FAIL
   only; it never serializes an environment value.

## 3. The denied set is mode-conditional

The keys the audit asserts absent are:

- the three `ENV_DENY_PREFIXES` (`MATRIX_`, `MX_AGENT_`, `ADW_`) — **always**
  denied, in every mode; **and**
- `GH_TOKEN` / `GH_BIN` — denied in **phased** mode (`allowGhToken: false`), but
  **intentionally present** in one-shot mode (`allowGhToken: true`).

The canonical audit target is the **phased** runner env (`allowGhToken: false`)
— the exact env real ADW runs use, built by the orchestrator as
`safeSubprocessEnv({ allowGhToken: false, runner: runner.id, source: deps.env })`
(`src/orchestrator.ts`). Asserting "`GH_TOKEN` absent" is therefore a statement
about *phased* mode, not a universal invariant; the test below pins the
one-shot exception so doc and code agree.

## 4. What is asserted, and the deterministic check

For the **phased** runner env (`allowGhToken: false`) of **each** runner
(`claude`, `codex`, `opencode`, `pi`):

- No key in the spawned child's `process.env` starts with any of
  `ENV_DENY_PREFIXES` (`MATRIX_`, `MX_AGENT_`, `ADW_`).
- `GH_TOKEN` and `GH_BIN` are absent.
- **Positive control (not trivially true):** the same poisoned source spawned
  **without** the allowlist — `{ env: POISONED }` — **does** expose the denied
  keys, proving the audit can actually observe a leak and that the clean result
  is the allowlist's doing, not the child being unable to read its env.
- **One-shot pin:** with `allowGhToken: true` the child *does* see
  `GH_TOKEN` / `GH_BIN` but **still** sees none of the `ENV_DENY_PREFIXES` keys.

Run the bundled, deterministic check:

```bash
cd adw_sdlc
npx vitest run test/secret-boundary-audit.test.ts
```

It is part of `npm run verify` (via `npm test`), needs **no real secrets** and
**no network**, spawns one trivial `node -e` child per case, and is safe in CI.
The deny prefixes come from `ENV_DENY_PREFIXES` in `src/env.ts` (single source
of truth) — the audit never re-hardcodes the list, so it tracks the boundary
automatically.

## 5. Operator spawn preflight (real env, no Claude call)

Before spending money, exercise the generic spawn boundary on a real operator box—where
`GH_TOKEN` / `MATRIX_*` may genuinely be set—using the **real** `process.env` as the source. This
spends no money and makes no agent call; it only spawns a trivial `node -e` child and prints
**names / booleans only, never values**. It is useful preflight, but it cannot satisfy an
"observed on a real runner" claim by itself:

```bash
cd adw_sdlc
npx tsx -e '
import { spawnSync } from "node:child_process";
import { ENV_DENY_PREFIXES, safeSubprocessEnv } from "./src/env.ts";
const SPECIFIC = ["GH_TOKEN", "GH_BIN"];               // phased-denied GitHub keys
const child =
  `const P=${JSON.stringify([...ENV_DENY_PREFIXES])},S=${JSON.stringify(SPECIFIC)};` +
  `process.stdout.write(JSON.stringify(Object.keys(process.env)` +
  `.filter(k=>P.some(p=>k.startsWith(p))||S.includes(k)).sort()))`;
for (const runner of ["claude", "codex", "opencode", "pi"]) {
  const env = safeSubprocessEnv({ allowGhToken: false, runner }); // real process.env as source
  const r = spawnSync(process.execPath, ["-e", child], { env, encoding: "utf8" });
  const leaked = JSON.parse(r.stdout);                 // NAMES only
  console.log(`${runner}: ${leaked.length === 0 ? "PASS" : "FAIL " + leaked.join(",")}`);
}
'
```

Expected output: a per-runner `PASS` and, on failure, the **names** of any
denied keys that reached the child (never values). A dedicated
`npm run audit:secrets` CLI is intentionally **deferred** — the snippet above is
the manual equivalent and the deterministic test (§4) is the CI-safe form.

## 6. Real Claude Agent SDK spawn probe

For real-runner evidence, point the supported `CLAUDE_BIN` override at
[`tools/claude-env-audit-wrapper.sh`](../tools/claude-env-audit-wrapper.sh) and put the actual
Claude executable in `CLAUDE_CODE_PATH`. Both names are in Claude's runner-specific allowlist. Use
a private `TMPDIR`, poison the parent, and carry the probe on one of the isolated
[`FAILURE-DRILLS.md`](./FAILURE-DRILLS.md) commands:

```bash
cd adw_sdlc
AUDIT_DIR="$(mktemp -d)"
chmod 700 "$AUDIT_DIR"
export TMPDIR="$AUDIT_DIR"
export CLAUDE_CODE_PATH="$(command -v claude)"
export CLAUDE_BIN="$PWD/tools/claude-env-audit-wrapper.sh"

# GH_TOKEN/GH_BIN are real parent inputs; the other families use non-secret sentinels.
# None of these commands prints a value.
export GH_TOKEN="$(gh auth token)"
export GH_BIN="$(command -v gh)"
export MATRIX_LIVE_AUDIT="SENTINEL-DENIED-MUST-NOT-APPEAR"
export ADW_LIVE_AUDIT="SENTINEL-DENIED-MUST-NOT-APPEAR"
export MX_AGENT_LIVE_AUDIT="SENTINEL-DENIED-MUST-NOT-APPEAR"

# Run one isolated command from FAILURE-DRILLS.md, then inspect the names-only record.
jq . "$AUDIT_DIR/claude-runner-env-audit.json"
unset GH_TOKEN GH_BIN MATRIX_LIVE_AUDIT ADW_LIVE_AUDIT MX_AGENT_LIVE_AUDIT
```

Use a fresh `AUDIT_DIR` per carrier call, or archive the JSON before another Claude spawn; the
fixed names-only evidence file is intentionally replaced on each invocation.

The boundary is exact:

1. The ADW gives `safeSubprocessEnv({ allowGhToken: false, runner: 'claude' })` to the Claude Agent
   SDK.
2. The SDK spawns `CLAUDE_BIN`; that executable is the probe, so it observes the environment of the
   real runner process—not a separately spawned `node -e` approximation.
3. The probe enumerates exported key **names** with Bash built-ins. If it sees `GH_TOKEN`, `GH_BIN`,
   `MATRIX_*`, `ADW_*`, or `MX_AGENT_*`, it records FAIL and exits `97` **before** invoking Claude.
4. On PASS it `exec`s `CLAUDE_CODE_PATH` with the identical argv and environment. Real Claude then
   produces the carrier run's normal transcript/error, proving the probe did not stop at a mock.

The SDK itself adds `CLAUDE_CODE_ENTRYPOINT` and `CLAUDE_AGENT_SDK_VERSION` before spawning the
executable. Those two observed control-key names are expected SDK metadata, not inherited parent
secrets; they are included in the names-only audit alongside `CLAUDE_BIN`, `CLAUDE_CODE_PATH`,
`HOME`, `PATH`, and `TMPDIR`.

### Observed 2026-07-16 evidence

The probe rode all three issue #20 live calls: timeout run `a6b4e6dc`, native-budget run
`b20d9e02`, and kill/resume run `c20e5a01`. Each real SDK spawn recorded `result: "PASS"` and an
empty `observed_denied_key_names` array despite the five poisoned parent key names. Claude then
produced the expected live timeout, budget, or phase result. A direct positive control that injected
`GH_TOKEN` into the wrapper environment exited `97` before Claude, demonstrating that PASS was not
vacuous.

Sanitized names-only reports and carrier summaries are archived in
[`test/fixtures/live-evidence`](../test/fixtures/live-evidence). These are the issue #21
observed-live artifact; the §5 Node preflight remains a reproducible CI-safe guard, not the evidence
used to claim a real spawned Claude runner.

## 7. Why this stays separate from `check-adw-sdlc-env.sh`

[`scripts/check-adw-sdlc-env.sh`](../../scripts/check-adw-sdlc-env.sh) is a
**static source** gate (no `...process.env` spread; opencode factory/import
rules) — it reasons about *source text* before any env is built. This audit is a
**runtime / spawn** check — it reasons about *what a real child observes*. They
catch different regressions: the lint catches a code pattern that *would* leak;
the audit catches an actual leak across the spawn boundary. They are kept in
separate files so each gate's intent stays legible. Do **not** fold this into
the bash lint.

## 8. Cross-references

- [`src/env.ts`](../src/env.ts) — the boundary (`ENV_DENY_PREFIXES`,
  `safeSubprocessEnv`).
- [`scripts/check-adw-sdlc-env.sh`](../../scripts/check-adw-sdlc-env.sh) — the
  static lint layer (`npm run lint:env`).
- [`test/env.test.ts`](../test/env.test.ts) — the mocked in-process unit layer.
- [`test/fixtures/live-evidence`](../test/fixtures/live-evidence) — sanitized evidence from real
  Claude carrier runs `a6b4e6dc`, `b20d9e02`, and `c20e5a01`.
- [`PARITY.md`](../PARITY.md) "Secret withholding (fail-closed)" row (Section 10)
  — the env-isolation guarantee this audit observes on a spawned child.
- [`docs/LIVE-RUN-BATCH.md`](./LIVE-RUN-BATCH.md) row 7 — the originating
  live-run batch item.
