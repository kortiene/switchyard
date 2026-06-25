# Secret-boundary audit (live-oriented, no secret printing)

How to assert, on a **real spawned runner environment**, that the denied
secrets — `GH_TOKEN` / `MATRIX_*` / `ADW_*` / legacy `MX_AGENT_*` — are absent,
**without ever printing a secret value** (key NAMES and pass/fail booleans
only).

- **Boundary audited:** [`src/env.ts`](../src/env.ts) — `ENV_DENY_PREFIXES`,
  `safeSubprocessEnv()`.
- **Deterministic check:** [`test/secret-boundary-audit.test.ts`](../test/secret-boundary-audit.test.ts)
  (runs inside `npm run verify`).
- **Advances:** [`MVP-READINESS.md`](../MVP-READINESS.md) §1 "operational basics"
  — moves the secret boundary from *statically linted* + *mocked-in-process* to
  *observed on a real spawned child*. Live-run batch row 7
  ([`docs/LIVE-RUN-BATCH.md`](./LIVE-RUN-BATCH.md)).

## 1. Why a third layer — the three layers of one boundary

| Layer | Where | What it proves | Limitation this audit addresses |
| --- | --- | --- | --- |
| **Static lint** | [`scripts/check-adw-sdlc-env.sh`](../../scripts/check-adw-sdlc-env.sh) (`npm run lint:env`) | No source file spreads `...process.env`; opencode factory/import rules | Reads *source text*, not a built env |
| **Mocked unit** | [`test/env.test.ts`](../test/env.test.ts) | `safeSubprocessEnv()` returns an object with no denied keys | Inspects the in-process **object**, never a spawned child |
| **Live audit (here)** | this doc + [`test/secret-boundary-audit.test.ts`](../test/secret-boundary-audit.test.ts) | A **child process spawned with that env** observes no denied key in its own `process.env` | — |

The distinguishing value over `env.test.ts` is that this audit crosses the
**spawn boundary**. It verifies that `spawnSync(bin, args, { env })` actually
**replaces** (does not merge) the parent environment on the running platform, so
a real runner grandchild (the agent's `bash` / `edit` / `write` tools) cannot
see an ambient secret. `env.test.ts` inspects only the in-process allowlist
object and so cannot catch a spawn-time merge regression; this audit can — e.g.
a future change that merged parent env, or a platform where `spawnSync` env
semantics differ.

This issue **scaffolds** the audit and ships its deterministic, CI-safe form. It
does **not** require a live `claude` / money-spending run; the operator
"live mode" against the real `process.env` (§5) is a documented, human-driven
step that spends no money and makes no agent call.

## 2. The no-secret-printing rule (load-bearing)

This is structural, not just a promise:

1. The audit reports **key NAMES and pass/fail booleans only** — never an
   environment **value**.
2. The deterministic check seeds the *source* env with **sentinels**, not real
   secrets: every denied key in the fixture holds the value
   `SENTINEL-DENIED-MUST-NOT-APPEAR`. So even a hypothetical bug that printed a
   value could only ever leak a sentinel, never a credential.
3. The spawned child emits the **names** of any denied keys it can see
   (expected: none). It never echoes `process.env` wholesale and never prints a
   value.
4. Operator **live mode** (real `process.env` as source, §5) inherits rules 1
   and 3: it still prints only the *names* of any leaked denied keys, so running
   it on a machine where a real `GH_TOKEN` / `MATRIX_*` is set cannot disclose
   the value even on failure.

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

## 5. Operator live mode (real env, human-driven)

To confirm the boundary on a real operator box — where `GH_TOKEN` / `MATRIX_*`
may genuinely be set — run the audit against the **real** `process.env` as the
source. This spends no money and makes no agent call; it only spawns a trivial
`node -e` child and prints **names / booleans only, never values**:

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

## 6. Why this stays separate from `check-adw-sdlc-env.sh`

[`scripts/check-adw-sdlc-env.sh`](../../scripts/check-adw-sdlc-env.sh) is a
**static source** gate (no `...process.env` spread; opencode factory/import
rules) — it reasons about *source text* before any env is built. This audit is a
**runtime / spawn** check — it reasons about *what a real child observes*. They
catch different regressions: the lint catches a code pattern that *would* leak;
the audit catches an actual leak across the spawn boundary. They are kept in
separate files so each gate's intent stays legible. Do **not** fold this into
the bash lint.

## 7. Cross-references

- [`src/env.ts`](../src/env.ts) — the boundary (`ENV_DENY_PREFIXES`,
  `safeSubprocessEnv`).
- [`scripts/check-adw-sdlc-env.sh`](../../scripts/check-adw-sdlc-env.sh) — the
  static lint layer (`npm run lint:env`).
- [`test/env.test.ts`](../test/env.test.ts) — the mocked in-process unit layer.
- [`PARITY.md`](../PARITY.md) "Secret withholding (fail-closed)" row (Section 10)
  — the env-isolation guarantee this audit observes on a spawned child.
- [`docs/LIVE-RUN-BATCH.md`](./LIVE-RUN-BATCH.md) row 7 — the originating
  live-run batch item.
