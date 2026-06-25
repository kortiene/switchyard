# Spec: live secret-boundary audit scaffold (no secret printing)

- **Work item:** GitHub issue #7 — `feat: live secret-boundary audit scaffold`
- **Labels / class:** `issue_class:feat`, `adw-live-batch`
- **Planned ADW run mode:** native (`docs/LIVE-RUN-BATCH.md` row 7)
- **Advances:** `MVP-READINESS.md` §1 "operational basics" — moves the secret boundary from
  *statically linted* + *mocked-in-process* to *observed on a real spawned child*.
- **Primary new file:** `adw_sdlc/docs/SECRET-BOUNDARY-AUDIT.md` (the procedure)
- **Optional helper (recommended):** `adw_sdlc/test/secret-boundary-audit.test.ts` (vitest;
  spawns a real child, names/booleans only) — keeps the audit inside `npm run verify`.
- **Source of truth referenced:** `adw_sdlc/src/env.ts` (`ENV_DENY_PREFIXES`, `safeSubprocessEnv`)
- **Status:** specification only. **Do not implement as part of this phase.**

---

## 1. Goal

Add a **live-oriented** way to assert that a *real spawned runner environment* — the env object
`safeSubprocessEnv()` hands a runner child — contains **none** of the denied secrets
(`GH_TOKEN` / `MATRIX_*` / `ADW_*` / `MX_AGENT_*`), and to do so **without ever printing a secret
value** (key NAMES and pass/fail booleans only).

This is the third layer of the same boundary, complementing the two that already exist:

| Layer | Where | What it proves | Limitation this issue addresses |
| --- | --- | --- | --- |
| **Static lint** | `scripts/check-adw-sdlc-env.sh` (`npm run lint:env`) | No source file spreads `...process.env`; opencode factory/import rules | Reads *source text*, not a built env |
| **Mocked unit** | `adw_sdlc/test/env.test.ts` | `safeSubprocessEnv()` returns an object with no denied keys | Inspects the in-process **object**, never a spawned child |
| **Live audit (this issue)** | `docs/SECRET-BOUNDARY-AUDIT.md` + optional `test/secret-boundary-audit.test.ts` | A **child process spawned with that env** observes no denied keys in its own `process.env` | — |

The distinguishing value over `env.test.ts` is that the audit crosses the **spawn boundary**: it
verifies that `spawnSync(bin, args, { env })` actually **replaces** (does not merge) the parent
environment on the running platform, so a real runner grandchild (the agent's `bash`/`edit`/`write`
tools) cannot see an ambient secret. `env.test.ts` cannot catch a spawn-time merge regression; this
audit can.

This issue **scaffolds** the audit and ships the deterministic, CI-safe form of it. It does **not**
require a live `claude`/money-spending run; the operator "live mode" against the real `process.env`
is a documented, human-driven step.

---

## 2. Background — verify before writing

Read these first; the spec's assertions depend on the exact current shapes.

### 2.1 The boundary being audited (`src/env.ts`)

- `ENV_DENY_PREFIXES = ['MATRIX_', 'MX_AGENT_', 'ADW_']` (`env.ts:39`) — never forwarded, even via
  `extraAllow` (the `extraAllow` loop at `env.ts:121-125` drops deny-prefixed keys).
- `safeSubprocessEnv(options)` (`env.ts:111-137`) builds the child env from `options.source ??
  process.env` by **copying only allowlisted keys that are present** — it never spreads the source.
  `GH_TOKEN`/`GH_BIN` are added **only** when `allowGhToken: true` (one-shot mode); phased ADW always
  passes `allowGhToken: false` (`env.ts:95-96` doc, and the orchestrator call site below).
- The orchestrator builds the runner-child env at `orchestrator.ts:1073`:
  `safeSubprocessEnv({ allowGhToken: false, runner: runner.id, source: deps.env })`. That is the
  exact call the audit reproduces.

### 2.2 How the env reaches the child (the thing being verified)

Each runner spawns with `{ env: req.env }` and nothing else — e.g. the pi adapter
(`src/runners/runner-pi.ts:279-285`, comment: *"The allowlist verbatim — the load-bearing D5
boundary; grandchildren … inherit this clean env"*). `spawnSync`/`spawn` with an explicit `env`
**replaces** the child environment (Node semantics; also asserted by the codex-spawn tests). The
audit's job is to confirm that replacement holds end-to-end, by reading the child's own
`process.env`.

### 2.3 The denied set is **mode-conditional** (state this in the doc)

The "denied keys" the audit asserts absent are:

- the three `ENV_DENY_PREFIXES` (`MATRIX_`, `MX_AGENT_`, `ADW_`) — always denied, every mode; **and**
- `GH_TOKEN` / `GH_BIN` — denied in **phased** mode (`allowGhToken: false`), **intentionally
  present** in one-shot mode (`allowGhToken: true`).

The canonical audit target is the **phased** runner env (`allowGhToken: false`), which is what real
ADW runs use and exactly the set the issue names. The doc must note that asserting "`GH_TOKEN`
absent" is a statement about *phased* mode, not a universal invariant.

### 2.4 Precedent to mirror

- `adw_sdlc/test/env.test.ts` — the `POISONED` source fixture (`:11-29`) and the
  deny-prefix-scan assertion style (`:36-40`). Reuse the *shape* of `POISONED`, with **sentinel**
  values (see §4.2).
- `adw_sdlc/test/runner-codex-spawn.test.ts` — precedent for a test that actually **spawns a
  child** and inspects its observed env; also imports `safeSubprocessEnv` from `../src/env.js`.
- `adw_sdlc/test/mvp-readiness-doc.test.ts`, `observed-live-ledger-doc.test.ts` — precedent for
  `readFileSync`-ing a committed doc and asserting on its content via `REPO_ROOT`
  (`../src/common.js`). Use this if a doc-content guard is wanted (§6, optional).
- `tools/parity-rate.ts` (wired to `npm run parity:rate`) — precedent for an operator-facing `tsx`
  tool under `tools/` with an npm script, if the optional CLI helper (§5.3) is built.

---

## 3. Scope

**In scope**

- **New doc** `adw_sdlc/docs/SECRET-BOUNDARY-AUDIT.md` — the audit procedure: what is asserted, the
  no-secret-printing rule, the deterministic check, and the operator live-mode command. *(Required.)*
- **Optional, recommended helper** `adw_sdlc/test/secret-boundary-audit.test.ts` — a deterministic
  vitest that spawns a real child with a poisoned-source allowlist env and asserts no denied key is
  visible in the child, printing only names/booleans. Runs inside `npm test` → `verify`.
- **Optional CLI** `adw_sdlc/tools/audit-secret-boundary.ts` + `npm run audit:secrets` — operator
  tool for the **live** mode (real `process.env` as source). Not part of `verify`.
- Minimal cross-links: one row in the README "Documentation map" table
  (`adw_sdlc/README.md:166-185`), and (if a test is added) the `HANDOVER.md` test-count bump the
  repo convention maintains (recent commits, e.g. `66b2ca2`, keep this count current).

**Out of scope**

- Any change to `src/env.ts`, `safeSubprocessEnv`, the allowlist contents, or the deny prefixes.
- Any change to runner adapters or how they spawn.
- Changing or extending `scripts/check-adw-sdlc-env.sh` (static lint stays as-is; this is the
  runtime/spawn-level complement, §7 explains why they stay separate).
- Running an actual live `claude` ADW run, or capturing live evidence (operator step).
- `.adw/` prompt-pack changes (so `pack:check` is untouched).

---

## 4. Required deliverable — the procedure doc

`adw_sdlc/docs/SECRET-BOUNDARY-AUDIT.md`. Recommended sections:

### 4.1 Purpose & the three layers

Open with the table from §1 (static lint / mocked unit / live audit) so a reader sees where this
fits and why a *spawned* check adds coverage the object-level test cannot.

### 4.2 The no-secret-printing rule (the load-bearing constraint)

State explicitly and make it structural, not just a promise:

1. The audit reports **key NAMES and pass/fail booleans only** — never an environment **value**.
2. The deterministic check seeds the *source* env with **sentinels**, not real secrets — e.g.
   `GH_TOKEN = 'SENTINEL-DENIED-MUST-NOT-APPEAR'`, `MATRIX_TOKEN = 'SENTINEL-…'`,
   `ADW_FOO = 'SENTINEL-…'`, `MX_AGENT_FOO = 'SENTINEL-…'`. So even a hypothetical bug that printed a
   value would leak a sentinel, never a credential.
3. The child emits the **names** of any denied keys it can see (expected: none) — it must never echo
   `process.env` wholesale and never print a value.
4. Operator **live mode** (real `process.env` as source) inherits rules 1 and 3: it still prints
   only the *names* of any leaked denied keys, so running it on a machine with a real `GH_TOKEN` set
   cannot disclose the token even on failure.

### 4.3 What is asserted

For the **phased** runner env (`allowGhToken: false`) of **each** runner (`claude`, `codex`,
`opencode`, `pi`):

- No key in the child's `process.env` starts with any of `ENV_DENY_PREFIXES`
  (`MATRIX_`, `MX_AGENT_`, `ADW_`).
- `GH_TOKEN` and `GH_BIN` are absent.
- (Positive control / not-trivially-true) the same poisoned source spawned **without** the
  allowlist — i.e. `{ env: POISONED }` — **does** expose the denied keys, proving the audit can
  actually observe a leak and that the clean result is the allowlist's doing.

### 4.4 The deterministic check (copy-paste)

Document running the bundled test:

```bash
cd adw_sdlc
npx vitest run test/secret-boundary-audit.test.ts
```

State that it is part of `npm run verify` (via `npm test`), needs no real secrets, no network, and
is safe in CI.

### 4.5 Operator live mode (real env, human-driven)

Document the live command (only if the optional CLI §5.3 is built; otherwise describe the manual
equivalent):

```bash
cd adw_sdlc
npm run audit:secrets -- --runner claude   # uses the real process.env as source; prints names/booleans only
```

Expected output: a per-runner `PASS` and, on failure, the **names** of any denied keys that reached
the child (never values). Note this is the form to run on a real operator box (where `GH_TOKEN` /
`MATRIX_*` may genuinely be set) to confirm the live boundary, and that it spends no money and makes
no agent call — it only spawns a trivial `node -e` child.

### 4.6 Cross-references

Link to `src/env.ts` (the boundary), `scripts/check-adw-sdlc-env.sh` (static lint),
`test/env.test.ts` (mocked unit), `PARITY.md` §10 (env-isolation guarantee), and back to
`docs/LIVE-RUN-BATCH.md` row 7.

---

## 5. Optional helper (recommended) — the deterministic spawned audit

### 5.1 Why a vitest test is the recommended helper form

- It keeps the audit **inside `npm run verify`** automatically (the issue's AC #3), with no operator
  step required to stay green.
- Spawning a child in a test is already precedented (`runner-codex-spawn.test.ts`,
  `verify-gate.e2e.test.ts`).
- It can import `ENV_DENY_PREFIXES` and `safeSubprocessEnv` from `../src/env.js` as the **single
  source of truth**, so the audit never re-hardcodes the prefix list (it tracks `env.ts`).
- It can include a **positive control** (negative-fixture) so the audit can't silently become a
  no-op (the "guard isn't trivially true" pattern used by the drift-guard and parity-core specs).

### 5.2 `test/secret-boundary-audit.test.ts` — design

Header comment: state this is the *live-oriented spawned* secret-boundary audit (the spawn-crossing
complement to `env.test.ts`'s in-process check) and the no-value-printing rule.

```ts
import { spawnSync } from 'node:child_process';
import { describe, expect, it } from 'vitest';
import { ENV_DENY_PREFIXES, safeSubprocessEnv } from '../src/env.js';
import type { RunnerId } from '../src/invoker.js';

// Sentinel-valued poisoned source: a value leaking would expose only a sentinel.
const POISONED: Record<string, string> = {
  HOME: '/home/u', USER: 'u', PATH: process.env['PATH'] ?? '/usr/bin',
  GH_TOKEN: 'SENTINEL-DENIED-MUST-NOT-APPEAR',
  GH_BIN: 'SENTINEL-DENIED-MUST-NOT-APPEAR',
  MATRIX_TOKEN: 'SENTINEL-DENIED-MUST-NOT-APPEAR',
  MX_AGENT_FOO: 'SENTINEL-DENIED-MUST-NOT-APPEAR',
  ADW_FOO: 'SENTINEL-DENIED-MUST-NOT-APPEAR',
};

const DENIED_SPECIFIC = ['GH_TOKEN', 'GH_BIN'] as const; // denied in phased mode

// Child reads its OWN process.env and prints ONLY the NAMES of denied keys it sees.
// Deny list is injected from the imported constant (single source of truth).
const CHILD = (prefixes: readonly string[], specific: readonly string[]) =>
  `const P=${JSON.stringify(prefixes)},S=${JSON.stringify(specific)};` +
  `const hits=Object.keys(process.env).filter(k=>P.some(p=>k.startsWith(p))||S.includes(k));` +
  `process.stdout.write(JSON.stringify(hits.sort()));`;

function deniedSeenByChild(env: Record<string, string>): string[] {
  const r = spawnSync(process.execPath, ['-e', CHILD(ENV_DENY_PREFIXES, DENIED_SPECIFIC)], {
    env, encoding: 'utf8',
  });
  expect(r.status, r.stderr).toBe(0);
  return JSON.parse(r.stdout) as string[];
}

const RUNNERS: RunnerId[] = ['claude', 'codex', 'opencode', 'pi'];
```

Tests:

1. **Per-runner phased env exposes no denied key (the audit).** For each runner, build
   `safeSubprocessEnv({ allowGhToken: false, runner, source: POISONED })`, spawn, assert
   `deniedSeenByChild(env)` is `[]`. On failure the message lists the **names** only (AC #1, #2).
2. **Positive control — the audit can see a leak.** Spawn with `{ env: POISONED }` directly (no
   allowlist) and assert the child *does* report the denied keys (the four sentinels' keys). Proves
   the clean result in test 1 is the allowlist's doing, not the child being unable to read env.
3. **No value ever crosses the boundary in the report.** Assert the child's stdout (and the
   assertion surface) contains key names only — specifically that it never contains the sentinel
   value substring `SENTINEL-DENIED-MUST-NOT-APPEAR`. (Captures the no-secret-printing AC #2
   structurally.)
4. **One-shot mode documents the GH_TOKEN exception.** Build with `allowGhToken: true` and assert
   the child *does* see `GH_TOKEN`/`GH_BIN` but **still** sees none of the `ENV_DENY_PREFIXES` keys.
   Pins §2.3's mode-conditional statement so the doc and code agree.

Strict-tsconfig notes: guard index access (`noUncheckedIndexedAccess`), no unused
locals/params (`noUnusedLocals`), and `JSON.parse` results are `unknown` → assert/cast narrowly.

### 5.3 Optional CLI for the operator live mode

If the maintainer wants a one-command operator tool (beyond the test), add
`adw_sdlc/tools/audit-secret-boundary.ts` and `"audit:secrets": "tsx tools/audit-secret-boundary.ts"`
to `package.json`. It mirrors the test's spawn logic but uses the **real** `process.env` as the
source (no `source:` override), accepts `--runner <id>` (default: all four), and prints
`PASS`/`FAIL` per runner plus, on failure, the **names** of leaked denied keys. It must reuse the
same `ENV_DENY_PREFIXES` import and the same names-only/never-values discipline. Keep it out of
`verify` (it inspects the live box, not a deterministic fixture). This is genuinely optional — the
doc can instead show the manual `node -e` equivalent.

---

## 6. Optional doc-content guard

If the repo wants the doc's invariants pinned (precedent: `mvp-readiness-doc.test.ts`), add a tiny
`describe` to `secret-boundary-audit.test.ts` that `readFileSync`s `docs/SECRET-BOUNDARY-AUDIT.md`
and asserts it (a) names all four denied tokens (`GH_TOKEN`, `MATRIX_`, `ADW_`, `MX_AGENT_`),
(b) states the names/booleans-only rule, and (c) links `src/env.ts`. Low value, low cost — include
only if matching the existing doc-test convention is desired. Default: **defer** unless the
maintainer keeps doc-content tests for every new doc.

---

## 7. Why this is separate from `check-adw-sdlc-env.sh`

`check-adw-sdlc-env.sh` is a **static source** gate (no `...process.env` spread; opencode
factory/import rules) — it reasons about *source text* before any env is built. This audit is a
**runtime/spawn** check — it reasons about *what a real child observes*. They catch different
regressions: the lint catches a code pattern that *would* leak; the audit catches an actual leak
across the spawn boundary (e.g., a future change that merged parent env, or a platform where
`spawnSync` env semantics differ). Keep them in separate files so each gate's intent stays legible.
Do **not** fold this into the bash lint.

---

## 8. Step-by-step implementation

1. Write `adw_sdlc/docs/SECRET-BOUNDARY-AUDIT.md` per §4 (the required deliverable). State the
   three layers, the no-value-printing rule, the mode-conditional denied set (§2.3), the
   deterministic command, and the operator live-mode command/manual equivalent.
2. (Recommended) Add `adw_sdlc/test/secret-boundary-audit.test.ts` per §5.2 (tests 1–4). Import the
   deny prefixes from `../src/env.js`; use **sentinel** values; print names only.
3. (Optional) Add `adw_sdlc/tools/audit-secret-boundary.ts` + `"audit:secrets"` script (§5.3) for
   the live operator mode.
4. Add a README "Documentation map" row for `docs/SECRET-BOUNDARY-AUDIT.md` (one line, additive).
5. If a test was added, bump the `HANDOVER.md` test-count line per repo convention (recent commits
   maintain it). Keep additive.
6. Focused run: `npx vitest run test/secret-boundary-audit.test.ts` from `adw_sdlc/`. Expect green,
   with test 2 (positive control) proving the audit fires.
7. Negative proof (manual, then revert): temporarily change test 1 to build the env with
   `{ env: POISONED }` (no allowlist) and confirm it **fails** listing the denied key NAMES (not
   values); revert. Confirms the audit is not trivially-true.
8. Full gate: `npm run verify` from `adw_sdlc/`.

No `src/` production changes. No `.adw/` changes (so `pack:generate`/`pack:check` are untouched).

---

## 9. Acceptance criteria

From the issue:

- [ ] Audit asserts denied prefixes/keys are absent from the runner env — **§5.2 test 1** (per
      runner, phased `allowGhToken: false`), backed by **§5.2 test 2** positive control.
- [ ] No secret values are ever printed (names/booleans only) — **§4.2** rule + **§5.2 test 3**
      (sentinel-value-never-in-output) + sentinel-valued `POISONED`.
- [ ] `npm run verify` stays green — the test is deterministic, no real secrets, no network; spawns
      one trivial `node -e` child; doc-only + test-only changes leave `typecheck`, `lint:env`,
      `pack:check`, `build` unaffected.

This spec adds:

- [ ] The audit reproduces the **exact** orchestrator call site
      (`safeSubprocessEnv({ allowGhToken: false, runner, source })`, `orchestrator.ts:1073`) and
      covers all four runners.
- [ ] The denied set's **mode-conditionality** (GH_TOKEN absent phased / present one-shot) is stated
      in the doc and pinned by **§5.2 test 4**.
- [ ] Deny prefixes come from `ENV_DENY_PREFIXES` (single source of truth), not a re-hardcoded list.
- [ ] `docs/SECRET-BOUNDARY-AUDIT.md` is discoverable (README doc-map row).

---

## 10. Risks & mitigations

| Risk | Mitigation |
| --- | --- |
| A bug prints a real secret value | Use **sentinel** source values + child emits NAMES only; §5.2 test 3 asserts the sentinel value never appears in output. Live mode (real env) still prints names only. |
| Audit is trivially-true (child can't read env, or filter never matches) | §5.2 test 2 positive control spawns `{ env: POISONED }` (no allowlist) and asserts the denied keys ARE seen; §8 step 7 manual negative proof. |
| Deny list drifts from `env.ts` | Import `ENV_DENY_PREFIXES` from `../src/env.js`; never re-hardcode the prefixes. |
| Platform injects non-secret env vars into the spawned child | Assert **denied keys absent**, not exact env equality — robust to any platform-added non-secret var. |
| Flaky/slow test from spawning | One short-lived `process.execPath -e` child per case (handful total); `spawnSync` is synchronous and fast; no network. |
| GH_TOKEN "must be absent" misread as universal | Doc §2.3 + §5.2 test 4 make the phased-vs-one-shot distinction explicit. |
| Strict tsconfig breaks the test build | Guard index access (`noUncheckedIndexedAccess`), avoid unused locals, narrow `JSON.parse` (`unknown`). Run `npm run typecheck`. |
| Scope creep (changing env.ts / lint / runners) | Explicitly out of scope (§3); this is a doc + read-only audit that imports the existing boundary. |

---

## 11. Open questions

- **O-1:** Ship the optional vitest helper, or doc-only? **Recommendation: ship the vitest helper**
  — it is the only form that satisfies AC #1/#2 *automatically inside `verify`* (a doc alone asserts
  nothing executable). The CLI (§5.3) and doc-content guard (§6) are genuinely optional.
- **O-2:** Build the operator CLI (`audit:secrets`) now, or document the manual `node -e` equivalent
  and defer the tool? Either satisfies the issue; defer if minimizing new surface.
- **O-3:** Should the live operator mode also exercise `allowGhToken: true` (one-shot) to confirm
  GH_TOKEN *is* present there? It documents the boundary fully but is not required by any AC; default
  to the phased assertion plus the §5.2 test-4 one-shot pin.
- **O-4:** Does the maintainer want a `PARITY.md` §10 row linking this live audit to the
  env-isolation guarantee, or is the README doc-map row + HANDOVER note sufficient? Default: doc-map
  + HANDOVER note only (additive, low-churn).
