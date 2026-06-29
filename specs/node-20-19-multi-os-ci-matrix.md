# Spec: Node-20.19 floor (+ optional multi-OS) CI matrix

- **Issue:** #37 — `Add a Node-20.19 / multi-OS CI matrix (engines floor never exercised)`
- **Labels:** `issue_class:ci`, `backlog`, `area:ci`
- **Intended `issue_class`:** `ci`
- **Planned ADW run mode:** native
- **Type:** CI / developer-tooling. A GitHub Actions workflow matrix + doc/comment
  edges + a workflow-guard test. **No production source (`src/`) changes** and
  (in the recommended primary scope) **no `package.json`/script changes.**
- **Owning files:** `.github/workflows/verify.yml` (matrix), optionally
  `adw_sdlc/test/coverage-config.test.ts` (workflow-guard assertions),
  `adw_sdlc/README.md` (`## Development`), `adw_sdlc/HANDOVER.md` (session entry +
  count baseline), `adw_sdlc/MVP-READINESS.md` / `adw_sdlc/PARITY.md` (the
  pi ≥22.19 note refresh).

---

## 1. Background & current state (read this first)

The two evidence points in the issue are **accurate** — confirm before acting:

- *"`adw_sdlc/.github/../verify.yml (single lane)`"* — **true.** The only CI
  workflow in the repo is `.github/workflows/verify.yml` (note: it lives at the
  **repo root** `.github/`, not under `adw_sdlc/`). It defines one job, `verify`,
  with a single fixed lane: `runs-on: ubuntu-latest`, `node-version: "22"`
  (`verify.yml:27-44`). There is no `strategy.matrix`. It runs `npm run verify`
  with `working-directory: adw_sdlc` and a `cache-dependency-path:
  adw_sdlc/package-lock.json`.
- *"`adw_sdlc/package.json engines >=20.19`"* — **true.** `package.json:7-9` is
  `"engines": { "node": ">=20.19" }`. CI therefore **never exercises the floor**:
  the only lane is Node 22.

### Facts verified in-repo (do not re-derive — confirm)

- **The floor is toolchain-imposed, not arbitrary.** `scaffold.test.ts:17-19`
  pins `pkg.engines.node === '>=20.19'` and documents why: "vitest 4's vite 8
  requires `^20.19.0 || >=22.12.0`." The lockfile echoes this on ~20
  vite/vitest sub-packages (`package-lock.json` rows
  `"node": "^20.19.0 || >=22.12.0"`). So `20.19.0` is the genuine minimum the
  toolchain claims to support — exactly the version "never exercised."
- **The matrix was always the design intent, never built.** `PLAN.md:152` (D3),
  `PLAN.md:692-695`, and `PLAN.md:718` all specify "CI matrix 20 + 22." This
  issue closes that gap. `PLAN.md:694-695` further notes: *"the pinned
  `@earendil-works/pi-coding-agent` declares `node >=22.19`, so the Node-20 lane
  exercises only the claude/codex/opencode adapters."*
- **pi's floor is higher than the package floor.**
  `node_modules/@earendil-works/pi-coding-agent/package.json` declares
  `"engines": { "node": ">=22.19.0" }`. pi is an **`optionalDependency`**
  (`package.json:30-33`). Repo comments already rely on a Node-20 lane *not*
  exercising pi:
  - `src/runners/runner-pi.ts:37-38`: "the npm package's engines floor (node
    >=22.19.0) makes the optionalDependency vanish on older Node installs."
  - `registry.test.ts:63-66`: the pi adapter imports **no SDK**, so it loads even
    where the optional dependency was skipped; a missing `pi` binary surfaces
    per-phase as a failed `PhaseResult`, never `RunnerNotInstalledError`.
  - `PARITY.md:72,118` and `MVP-READINESS.md:116-118` already say in prose: "pi
    needs Node ≥ 22.19 … the CI node-20 lane skips it." Today that sentence
    describes a lane that **does not exist yet** — this issue makes it real.
- **CI merge-gating is check-name-agnostic.** The ADW orchestrator does not look
  for a check literally named `verify`. `orchestrator.ts:511-545` polls an
  **aggregate** `providers.changeRequests.pipelineStatus(...)` that returns
  `success` only when *all* checks are green (`pending`/`failure`/`none`/`unknown`
  otherwise). Expanding one job into N matrix legs ("verify (node 20.19.0)",
  "verify (node 22)") therefore keeps the orchestrator's pre-merge gate correct
  with **no code change** — it waits for all legs.
- **There is already a workflow-guard test.** `coverage-config.test.ts:118-134`
  reads `../../.github/workflows/verify.yml` and asserts it (a) contains
  `npm run verify` and (b) contains `working-directory: adw_sdlc`. **The
  recommended matrix design keeps both literals**, so these stay green without
  edits — but the matrix should add its own guard assertions (§3.4).
- **The `verify` chain has OS-specific steps** (relevant only to the *multi-OS*
  axis, §5/Alt-A):
  - `package.json:16` `verify` ends with **`rm -rf dist`** — POSIX-only; `rm` does
    not exist in Windows `cmd.exe`/PowerShell (npm's default script shell on
    Windows). **This breaks `npm run verify` on `windows-latest`.**
  - `package.json:15` `lint:env` = **`bash ../scripts/check-adw-sdlc-env.sh`**,
    a `set -euo pipefail` + `grep` script. It needs `bash`; Git Bash exists on
    GitHub `windows-latest` runners but is not the default, so this is fragile.
  - macOS (`macos-latest`) is POSIX — `bash`, `rm`, `grep` all present — so the
    gate runs there **unchanged**.
- **Cost note (multi-OS only):** GitHub-hosted `macos-*` minutes bill ~10× and
  `windows-*` ~2× Linux minutes; an OS axis multiplies the lane count.
- **Doc-guard tests to keep green** if docs are touched:
  `handover-doc.test.ts` (a "Test count baseline after this session: **NNN
  passing across MM files**" line must exist, with `NNN ≥ 600` and `MM ≥ 43`);
  `mvp-readiness-doc.test.ts` (structural `## 0.`/`(A)`/`(B)`/`(C)`/`post-MVP`
  matches — a wording refresh on the pi line at `MVP-READINESS.md:117` is safe as
  long as those anchors remain).

---

## 2. Goal & acceptance criteria

### Goal
Actually run the test suite on the **Node `20.19` engines floor** in CI (today it
is only ever run on Node 22, so a 20.19-only breakage would ship silently),
alongside the existing Node 22 lane, and **document** that the `pi` runner needs
Node ≥ 22.19 and is therefore exercised only on the ≥22 lane.

### Acceptance criteria (from the issue)
- [ ] **CI matrix covers the Node 20.19 floor alongside 22** — `verify.yml`
      defines a `strategy.matrix` running the full `npm run verify` gate on both
      Node `20.19.0` (the engines floor) and Node `22`, both green.
- [ ] **Document the pi ≥22.19 lane** — it is recorded (workflow comment + README,
      reconciled with the existing `PARITY.md`/`MVP-READINESS.md` prose) that the
      `pi` runner requires Node ≥ 22.19, so the Node-20.19 lane does not / cannot
      exercise pi, and only the ≥22 lane can.

### Additional acceptance criteria (this spec)
- [ ] **The floor is exercised literally.** The low lane pins `20.19.0` (the exact
      `engines` minimum), not "latest 20.x", so the *floor* — the thing the issue
      says is never tested — is the thing that runs.
- [ ] **`npm ci` succeeds on the Node-20.19 lane** despite pi's `>=22.19.0`
      engines (pi is optional + non-strict; see §8 risk). The mocked suite passes
      on Node 20.19 with no pi binary present.
- [ ] **Both legs are independent signal.** `fail-fast: false` so a 20.19-only or
      22-only failure does not mask the other lane's result.
- [ ] **The existing workflow-guard test stays green** (`coverage-config.test.ts`
      still finds `npm run verify` and `working-directory: adw_sdlc`), and a new
      matrix-guard assertion locks the floor lane so it can't silently regress.
- [ ] **The orchestrator pre-merge gate still works** unchanged (it aggregates all
      legs; §1). No `src/` change.
- [ ] **Multi-OS scope is an explicit, recorded decision** (§5): either deferred
      to a follow-up with the portability blockers named, or, if taken now, the
      `rm -rf dist`/`bash` blockers are resolved first.

---

## 3. Recommended design (primary): Node-version matrix on ubuntu

Add a two-entry **Node-version matrix** to the existing `verify` job, keep
`runs-on: ubuntu-latest`, and leave `package.json`/scripts untouched. This is the
smallest change that satisfies both acceptance criteria, matches the long-standing
`PLAN.md` design intent (20 + 22), and avoids the Windows portability work (§5).
The "multi-OS" half of the title is handled as a **documented decision** (§5),
recommended for a follow-up.

### 3.1 `.github/workflows/verify.yml` — add the matrix

Replace the single-lane `verify` job with a matrixed one. Concretely:

```yaml
jobs:
  verify:
    # One leg per supported Node line. The low leg pins the package engines
    # floor (>=20.19) — previously never exercised in CI (#37); the high leg is
    # the local-dev / pi line (pi requires Node >=22.19, so only this leg can
    # exercise the pi runner — see README "Development").
    name: verify (node ${{ matrix.node }})
    runs-on: ubuntu-latest
    strategy:
      # Report every leg independently; a 20.19-only break must not be masked
      # by the 22 leg (and vice-versa).
      fail-fast: false
      matrix:
        node: ["20.19.0", "22"]
    defaults:
      run:
        working-directory: adw_sdlc
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: ${{ matrix.node }}
          cache: npm
          cache-dependency-path: adw_sdlc/package-lock.json

      # Reproducible install from the committed lockfile. The pi optionalDependency
      # declares engines node>=22.19.0; on the 20.19.0 leg npm emits an EBADENGINE
      # WARNING and continues (engine-strict is unset). The suite is fully mocked
      # and never spawns pi, so it is green on both legs.
      - name: Install (npm ci)
        run: npm ci

      # typecheck -> lint:env -> pack:check -> mirror:check -> coverage -> build -> rm -rf dist
      - name: Verify
        run: npm run verify
```

Key choices (and why):

- **Pin `20.19.0`, not `20` or `20.x`.** The issue's whole point is that the
  *floor* is never exercised. `setup-node` with `"20"` resolves to the latest
  20.x, which would test a higher patch than the declared minimum. `"20.19.0"`
  runs the literal `engines` floor. (`setup-node` accepts the exact version and
  pulls it from its version manifest.)
- **`"22"` for the high leg** (latest 22.x ≥ 22.19) — keeps parity with local dev
  and satisfies pi's `>=22.19.0` floor, so the 22 leg is the one capable of
  exercising pi. (Optionally pin `"22.19.0"` to make "the pi lane" maximally
  explicit; latest-`22` already satisfies it and needs less maintenance — see
  Open Question 2.)
- **`fail-fast: false`** — independent signal per Node line; the floor leg failing
  is the *high-value* outcome this issue is buying, so it must not be cancelled by
  an unrelated 22 failure.
- **`name: verify (node ${{ matrix.node }})`** — gives each leg a stable, readable
  check name for branch protection (§9) and PR UX.
- **No `package.json` change** — `verify` is run verbatim on both legs. (Contrast
  the multi-OS axis, which *does* require script changes — §5.)
- **Cache**: one `cache-dependency-path` (same lockfile) is shared across legs;
  `setup-node` keys the cache by runner+arch+lockfile hash, so the two Node legs
  coexist without collision. No change needed.
- **`concurrency` block stays as-is** (`group: verify-${{ github.ref }}`,
  `cancel-in-progress: true`) — still "one in-flight run per ref"; it now cancels
  the whole matrix on a newer push, which is the desired behavior.

### 3.2 Update the workflow header comment

The top-of-file comment (`verify.yml:3-11`) describes a single lane ("Runs the
same single command…"). Update it to state: (a) the gate now runs on a Node
**20.19.0 (engines floor) + 22** matrix, (b) why the floor leg exists (#37 —
previously never exercised), and (c) the **pi ≥22.19** fact: the Node-20.19 leg
cannot exercise the pi runner; only the ≥22 leg can. This single comment edit is
the primary on-ramp for the "Document the pi ≥22.19 lane" AC; README + the
existing PARITY/MVP-READINESS prose (§3.5) complete it.

### 3.3 No production-source or package changes

`src/**`, `tsconfig*.json`, `vitest.config.ts`, `package.json`, and
`package-lock.json` are **untouched** in the primary scope. `pack:check` /
`mirror:check` therefore cannot drift (no prompt source changed).

### 3.4 Tests — extend the workflow guard

`coverage-config.test.ts:118-134` already guards `verify.yml`. The recommended
matrix keeps `npm run verify` and `working-directory: adw_sdlc`, so the two
existing assertions **stay green unmodified**. Add a sibling `describe` (do not
mutate the `#36` block) that locks the new behavior so a future edit can't quietly
drop the floor leg:

```ts
describe('CI workflow — Node version matrix (issue #37)', () => {
  let workflowSource: string;
  beforeAll(() => {
    workflowSource = readFileSync(
      new URL('../../.github/workflows/verify.yml', import.meta.url), 'utf8');
  });

  it('runs a Node-version matrix', () => {
    expect(workflowSource).toMatch(/strategy:/);
    expect(workflowSource).toMatch(/matrix:/);
  });

  it('exercises the package engines floor (20.19.0) alongside 22', () => {
    // The floor is the thing #37 says was never tested — pin it literally.
    expect(workflowSource).toContain('20.19.0');
    expect(workflowSource).toMatch(/["']22(\.\d+\.\d+)?["']/);
  });

  it('drives Node selection from the matrix var', () => {
    expect(workflowSource).toContain('node-version: ${{ matrix.node }}');
  });

  it('keeps legs independent (fail-fast: false)', () => {
    expect(workflowSource).toMatch(/fail-fast:\s*false/);
  });

  it('documents that pi needs Node >= 22.19 (the floor leg cannot run pi)', () => {
    // Satisfies the "Document the pi >=22.19 lane" AC at the guard level.
    expect(workflowSource).toMatch(/22\.19/);
  });
});
```

This is a **text guard** in the established repo convention (the repo already
text-asserts `verify.yml`, README, HANDOVER, MVP-READINESS prose). It does **not**
execute the workflow.

> Adding this `describe` raises the suite's test/file counts. If the HANDOVER
> "Test count baseline" line is updated (§3.5), keep it `≥ 600 / ≥ 43` so
> `handover-doc.test.ts` stays green; record the **actual** observed numbers.

### 3.5 Docs to update (for the "Document the pi ≥22.19 lane" AC)

Minimal, blast-radius-aware edits:

- **`adw_sdlc/README.md` `## Development`** — add one or two sentences: CI runs
  `npm run verify` on a **Node 20.19.0 (engines floor) + 22** matrix; the `pi`
  runner requires Node ≥ 22.19, so only the ≥22 leg can exercise it (the 20.19.0
  leg covers claude/codex/opencode). Keep the existing `npm run verify`,
  `ADW_TEST_CMD="npm run verify"`, and `LIVE-RUN-BATCH.md` references intact
  (`mvp-readiness-doc.test.ts:56,60-61` and the README asserts depend on them).
- **`adw_sdlc/MVP-READINESS.md:116-118`** — the checkbox currently reads "pi needs
  Node ≥ 22.19 (the CI node-20 lane skips it) — bump CI or accept pi is unverified
  in CI." Refresh to reflect that the node-20.19 lane now **exists** and pi is
  exercised on the ≥22 leg (or remains live-unverified, whichever is accurate).
  Preserve the section's structural anchors (`post-MVP`, `(A)/(B)/(C)`, `❌`) so
  `mvp-readiness-doc.test.ts` stays green.
- **`adw_sdlc/PARITY.md:72,118`** — the prose "the CI node-20 lane skips pi" is
  now literally true; optionally tighten to "the CI node-20.19 lane (now present)
  skips pi; pi is exercised on the Node-22 leg." Low priority.
- **`adw_sdlc/HANDOVER.md`** — add a session entry in the established format
  (an `## 8x. Issue #37 — Node-floor CI matrix` block) noting: matrix added
  (20.19.0 + 22, fail-fast:false), pi-≥22.19 documented, workflow-guard test
  added, and the new **Test count baseline** line with the observed counts.
- **`adw_sdlc/PLAN.md:718`** (optional) — PLAN already *specifies* the 20+22
  matrix as design intent; a one-line "implemented in #37" note keeps the design
  doc honest but is not required by the AC.

---

## 4. Files to change

| File | Change | Scope |
| --- | --- | --- |
| `.github/workflows/verify.yml` | Add `strategy.matrix.node: ["20.19.0","22"]`, `fail-fast: false`, `name: verify (node …)`, drive `node-version` from `matrix.node`; refresh header comment incl. the pi ≥22.19 note. | **required** |
| `adw_sdlc/test/coverage-config.test.ts` | Add an issue-#37 `describe` asserting matrix/floor/fail-fast/pi-note (§3.4). | recommended |
| `adw_sdlc/README.md` | `## Development`: matrix + pi ≥22.19 sentence. | required (AC2) |
| `adw_sdlc/MVP-READINESS.md` | Refresh the pi-≥22.19 checkbox (`:116-118`). | recommended (AC2) |
| `adw_sdlc/PARITY.md` | Tighten the "node-20 lane skips pi" prose (`:72,118`). | optional |
| `adw_sdlc/HANDOVER.md` | Session entry + updated Test count baseline. | recommended (repo convention) |

**Not touched (primary scope):** any `src/*.ts`, `package.json`,
`package-lock.json`, `vitest.config.ts`, `tsconfig*.json`,
`scripts/check-adw-sdlc-env.sh`, `.adw/`/`.pi/`/`.claude/` prompt sources.

---

## 5. Multi-OS axis — analysis & recommended decision

The issue **title** says "multi-OS", but the **acceptance criteria do not** — they
name only the Node 20.19 floor and the pi doc. Treat OS as a separate, explicit
decision rather than silently shipping or silently dropping it.

**Blockers for `windows-latest`** (must be fixed *before* adding a Windows leg, or
the gate is red-by-construction):
1. `verify` ends with `rm -rf dist` → no `rm` in `cmd.exe`/PowerShell. Fix:
   replace with a cross-platform delete (`rimraf` devDep, or `node -e
   "fs.rmSync('dist',{recursive:true,force:true})"`). Touches `package.json` +
   lockfile + the `scaffold.test.ts` `verify`-chain guard (which pins the literal
   `rm -rf dist` tail).
2. `lint:env` = `bash ../scripts/check-adw-sdlc-env.sh` → needs `bash`. Git Bash
   is on `windows-latest` but isn't npm's default script shell; you'd pin
   `setup-node`/`shell: bash`, or port the lint to a Node script. Non-trivial.

**`macos-latest`** has **no blockers** — it is POSIX (`bash`, `rm`, `grep` all
present), so `npm run verify` runs there unchanged. A `macos-latest` leg is the
cheap, honest way to add "multi-OS" coverage without script surgery (cost caveat:
macOS minutes bill ~10×).

**Recommendation (primary):** ship the **Node-version matrix on `ubuntu-latest`
only** (§3) to satisfy both ACs now, and **defer Windows to a follow-up issue**
that does the `rm -rf`/`bash` portability work first. Optionally add a
`macos-latest` leg in the same PR if real cross-OS coverage is wanted immediately
(POSIX, zero script change) — but be deliberate about the minute cost and the leg
explosion (a 2-Node × 2-OS matrix = 4 jobs). Use `matrix.include` to add *only*
the OS legs you want rather than a full cross-product (e.g. Node 20.19.0 + 22 on
ubuntu, and just one of them on macOS) to keep the lane count and cost down.

This keeps the change small and auditable (working rules) while addressing the
title honestly via a recorded decision rather than scope creep.

---

## 6. Step-by-step implementation

> The crux of this issue is **proving the floor actually works** — do the local
> Node-20.19.0 run *first*; if it fails, that failure *is* the bug the issue
> exists to surface (capture it; it may spawn a fix sub-task).

### Step 1 — Reproduce the floor locally (the real test of this issue)
From `adw_sdlc/`, using the exact floor:
```bash
nvm install 20.19.0 && nvm use 20.19.0   # or fnm/volta equivalent
node -v                                   # expect v20.19.0
npm ci                                    # expect success; EBADENGINE WARNING for pi is OK
npm run verify                            # expect GREEN
```
If `npm ci` *errors* (not warns) on pi's engines, see §8 (do **not** add
`--no-optional`; that would drop all four SDKs and break other runner tests).
If `npm run verify` is red on 20.19.0, record the exact failure — that is the
previously-hidden floor breakage; resolve it (or file a blocking sub-issue) before
the matrix can be merged green.

### Step 2 — Add the matrix
Apply the `verify.yml` changes from §3.1–§3.2 (matrix, `fail-fast: false`, job
name, `node-version: ${{ matrix.node }}`, header comment incl. the pi note).

### Step 3 — Add/extend the workflow-guard test
Add the issue-#37 `describe` from §3.4 to
`adw_sdlc/test/coverage-config.test.ts`. Run it focused:
```bash
npx vitest run test/coverage-config.test.ts
```

### Step 4 — Docs
Update README `## Development` (matrix + pi ≥22.19), refresh
`MVP-READINESS.md:116-118` / `PARITY.md` prose, and add the HANDOVER session entry
(§3.5). If you touch HANDOVER's baseline line, set it to the **observed** counts
(`≥ 600 / ≥ 43`).

### Step 5 — Full gate on the dev Node (22) and the floor (20.19.0)
```bash
npm run verify        # on 22.x (local dev)
nvm use 20.19.0 && npm run verify   # confirm the floor leg is green too
```
Expect green on both, `dist/` removed, no `pack:check`/`mirror:check` drift, and
`coverage/` git-ignored.

### Step 6 — (Decision) Multi-OS
If the maintainer opts in now: either add a `macos-latest` leg (no script change),
or do the §5 Windows portability work first. Otherwise file the deferral
follow-up and note it in the PR description / HANDOVER.

### Step 7 — Branch-protection migration (repo admin; out-of-band)
See §9 — the required-check **names change**; a repo admin must update branch
protection. The implementing ADW phase **does not** touch GitHub settings
(orchestrator/admin concern).

---

## 7. Test & verification strategy

- **The verification *is* the feature:** `npm run verify` green on **Node 20.19.0**
  (the lane that never ran before) is the primary evidence. Run it locally on
  20.19.0 before relying on CI.
- **Workflow-guard unit test:** `npx vitest run test/coverage-config.test.ts`
  (existing #36 assertions + new #37 matrix assertions).
- **Full local gate (both Node lines):** `npm run verify` on 22.x and on 20.19.0.
- **CI confirmation:** the PR shows **two** required check legs — "verify (node
  20.19.0)" and "verify (node 22)" — both green; the ADW orchestrator's aggregate
  CI poll (`orchestrator.ts:511-545`) sees all-green before gating the merge.
- **Negative check (manual, do not commit):** temporarily break something
  20.19-specific (or set the low leg to a version the toolchain rejects) and
  confirm *only* that leg goes red while the 22 leg stays green — proving
  `fail-fast: false` gives independent signal.
- **No execution of the workflow from a test** (would need the Actions runtime);
  the guard is a static text assertion, matching repo convention.

---

## 8. Risks & mitigations

| Risk | Likelihood | Mitigation |
| --- | --- | --- |
| **The floor lane is actually red** (toolchain/tsx/vitest/coverage-v8 misbehaves on 20.19.0 despite `engines`). | Medium | This is the value of the issue. Step 1 runs it locally first; if red, fix or file a blocking sub-issue **before** merge. Don't paper over by raising the floor without sign-off. |
| `npm ci` **errors** on the 20.19.0 leg because pi declares `node>=22.19.0`. | Low–Medium | pi is an **optionalDependency** and `engine-strict` is unset → npm emits an EBADENGINE **warning** and continues (this is the documented assumption in `runner-pi.ts:37-38`/`PLAN.md:954`). Verify in Step 1. If it genuinely fails, the fix is targeted (e.g. tolerate the optional-dep skip), **not** `--no-optional`. |
| **Branch protection breaks**: the required check `verify` no longer reports (renamed to `verify (node …)`), so PRs block forever or lose their gate. | High if unmanaged | §9: a repo admin must swap the required-status-check names. Call this out in the PR description; it's an out-of-band admin step, not a code change. |
| Existing `coverage-config.test.ts` #36 assertions break. | Low | The matrix **keeps** `npm run verify` and `working-directory: adw_sdlc`; the new `describe` is additive. Run the test focused (Step 3). |
| `handover-doc.test.ts` / `mvp-readiness-doc.test.ts` go red on doc edits. | Medium | Keep HANDOVER's baseline line `≥600/≥43` with real counts; preserve MVP-READINESS structural anchors (`## 0.`, `(A)/(B)/(C)`, `post-MVP`, `❌`). |
| Multi-OS (if taken now) red-by-construction on Windows. | High (Windows) | §5: fix `rm -rf dist` + `bash` lint **first**, or scope OS to ubuntu (+ optional macOS, which is POSIX). |
| CI minute cost balloons with an OS cross-product. | Medium (multi-OS only) | Use `matrix.include` to add only chosen legs; macOS bills ~10×. Default primary scope adds just **one** extra ubuntu leg. |
| `cancel-in-progress` now cancels a whole matrix mid-flight on a new push. | Low | Intended (one in-flight run per ref); legs are fast and fully mocked. |

This is a low-risk CI item. The only genuine unknown is whether the floor lane is
actually green — which is precisely what the issue wants surfaced.

---

## 9. Rollout / rollback

- **Rollout:** add the matrix + guard test + docs. No runtime behavior change, no
  migration, no feature flag. The two legs begin reporting on the next PR/push.
- **Branch-protection migration (REQUIRED, out-of-band, repo admin):** GitHub
  required status checks are matched by **context name**. Today the required check
  is likely `verify` (or `verify / verify`). After this change the contexts become
  `verify (node 20.19.0)` and `verify (node 22)`. An admin must update branch
  protection on `main` to require the **new** names (ideally **both** legs), and
  remove the stale `verify` requirement. Until then a PR may either block on a
  check that never reports or merge without the gate. The ADW orchestrator's own
  poll is name-agnostic and needs no change (§1).
- **Rollback:** revert `verify.yml` to the single `node-version: "22"` lane (and
  drop the #37 guard `describe`). Nothing else depends on the matrix; re-point
  branch protection back to the single `verify` context.
- **Forward (multi-OS):** a later issue adds the OS axis after the §5 portability
  fixes — out of scope here unless explicitly opted in.

---

## 10. Key decisions

1. **Node-version matrix on ubuntu is the primary scope** — it satisfies both ACs
   with the smallest, most auditable change and matches `PLAN.md`'s long-standing
   "20 + 22" intent. Multi-OS is a separate recorded decision (§5).
2. **Pin the low leg to `20.19.0` (exact floor), not `20`/`20.x`.** The issue is
   that the *floor* is never run; run the literal floor.
3. **`"22"` for the high leg** (≥ pi's 22.19 floor) — this is the only leg that can
   exercise the pi runner; document that. (Pinning `"22.19.0"` is an option — OQ2.)
4. **`fail-fast: false`** — independent signal; a floor-only break is the
   high-value outcome and must not be masked.
5. **No `package.json`/script change in the primary scope** — `verify` runs
   verbatim; OS portability (which *would* require script changes) is deferred.
6. **Keep the workflow check-name-agnostic for the orchestrator**, but acknowledge
   the **branch-protection** rename as an out-of-band admin migration (§9).
7. **Document the pi ≥22.19 fact in three coherent places** (workflow comment,
   README, and the existing PARITY/MVP-READINESS prose) so they stop describing a
   lane that didn't exist and start describing the one that now does.
8. **Recommend deferring Windows; offer macOS as the zero-script-change "multi-OS"
   option** if cross-OS coverage is wanted now.

---

## 11. Assumptions

- The implementing/CI environment can obtain **Node 20.19.0** (`setup-node`'s
  manifest provides it; `nvm`/`fnm`/`volta` locally).
- `npm ci` on Node 20.19.0 **succeeds with a warning** for pi's engines (optional
  dep, `engine-strict` unset) — consistent with `runner-pi.ts:37-38` and
  `PLAN.md:954`. To be confirmed in Step 1.
- The full mocked suite **passes on 20.19.0** (no pi binary, no network/API keys —
  `verify.yml:46-47`). If it does not, the issue has surfaced a real floor bug to
  fix (in scope to identify, possibly out of scope to fix depending on cause).
- `verify.yml` lives at the **repo root** `.github/` and is the **only** workflow;
  there is no separate Python `adw` job in this standalone port (PLAN's mention of
  one is historical). The package manager is **npm** (`package-lock.json` +
  `npm ci`), not the pnpm referenced in PLAN.
- The orchestrator's `pipelineStatus` aggregates *all* PR checks (so N legs gate
  correctly); confirmed against `orchestrator.ts:511-545`.
- A repo admin will perform the branch-protection rename (§9); the ADW phase has
  no GitHub-settings access and must not attempt it.
- Touching docs is welcome (repo convention); if the maintainer wants zero
  doc/test churn, the two ACs are still met by the `verify.yml` change alone
  (matrix + the pi note in the header comment).

---

## 12. Open questions

1. **Multi-OS now or follow-up?** Primary recommends ubuntu-only now + a deferral
   issue for Windows (needs `rm -rf dist`/`bash` portability work first). Add a
   `macos-latest` leg in this PR (POSIX, no script change, ~10× minutes), or keep
   it ubuntu-only? **Recommendation:** ubuntu-only now; defer Windows; macOS
   optional.
2. **High-leg pin: `"22"` (latest) vs `"22.19.0"` (pi's exact floor)?** Latest-22
   already satisfies pi and needs no maintenance; pinning `22.19.0` makes "the pi
   lane" maximally explicit but freezes a patch. **Recommendation:** `"22"`.
3. **Should the 20.19.0 leg be allowed to fail (`continue-on-error`) initially**
   if the floor turns out red, landing the matrix as informational until a fix —
   or must the floor be green before merge? **Recommendation:** green before merge
   (an always-yellow floor leg defeats the purpose); file a blocking sub-issue if
   the floor is broken.
4. **Branch-protection ownership.** Who updates the required-status-check names on
   `main` (§9), and should *both* legs be required (recommended) or just the floor?
5. **pi live-verification follow-through.** `MVP-READINESS.md:117` ties this to
   "bump CI or accept pi is unverified." Does landing the 22 leg (which *can* run
   pi) change that checkbox's status, or is a separate live pi run still owed
   (`PARITY.md:118`)? Clarify the exact wording so the doc-guard tests and the
   MVP ledger stay consistent.
6. **Do we want `npm ci`'s EBADENGINE pi warning silenced** on the 20.19.0 leg
   (e.g. a step note) to avoid alarming readers, or leave it visible as honest
   signal? **Recommendation:** leave it visible; it documents the pi floor in the
   logs themselves.
