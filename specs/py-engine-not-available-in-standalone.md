# Spec: decide the fate of the non-functional `--engine py` path

- **Work item:** GitHub issue #27 — _Decide the fate of the non-functional `--engine py` path in the standalone port_
- **Labels / class:** `issue_class:fix`, `area:cli`, `backlog`
- **Source:** `adw_sdlc/src/cli.ts:42-48,357-390`; `adw/` (no `issue.py` present); backlog assessment (PLAN.md §11 / MVP-READINESS.md / PARITY.md)
- **Status:** specification only. **Do not implement as part of this phase.**

---

## 1. Goal

Make the `py` engine path **honest**. Today `--engine py` / `ADW_ENGINE=py` is an
advertised, selectable mode that *cannot work* in this standalone TypeScript-only
port: it spawns `python3 adw/issue.py`, a file that does not exist here
(`adw/` ships only `state.schema.json` + `fixtures/`). The result is a confusing
runtime failure that depends on whether `python3` is even installed:

- `python3` absent → `error: could not launch the py engine (python3): spawn python3 ENOENT` (rc 1, `cli.ts:378-380`).
- `python3` present → it runs, fails to open the missing script, and returns whatever rc the interpreter chooses (typically `2`), with a raw Python `can't open file` message on stderr (`cli.ts:381-384`).

Neither outcome tells the operator the real cause: **this distribution does not
bundle the Python engine.** The fix replaces the dead spawn with an explicit,
deterministic `AdwError`, and aligns the docs/help so the mode is never presented
as functional.

---

## 2. Background — verify before writing

### 2.1 What exists today

| Element | Location | Behavior |
| --- | --- | --- |
| `ENGINE_IDS` / `EngineId` | `cli.ts:38-39` | `['py', 'ts']` — both recognized |
| `DEFAULT_ENGINE` | `cli.ts:48` | `'ts'` (cutover already done in this port) |
| `resolveEngineId(raw)` | `cli.ts:55-63` | empty → default `ts`; `'py'`/`'ts'` accepted; anything else throws `unknown engine: '<x>' (valid: py, ts)` |
| `CliDeps.runPyEngine` | `cli.ts:356-358` | injectable seam: `(argv) => Promise<number>` |
| `spawnPyEngine(argv)` | `cli.ts:369-385` | spawns `python3 <REPO_ROOT>/adw/issue.py …`, `cwd=REPO_ROOT`, `stdio:'inherit'`, **no `env:`** (child inherits full parent env on purpose) |
| `defaultCliDeps()` | `cli.ts:387-394` | wires `runPyEngine: spawnPyEngine` |
| `main()` py branch | `cli.ts:432-435` | `if (engine === 'py') { forward argv (minus `--engine`, post-`--` re-appended) → deps.runPyEngine }` |
| Imports used **only** by the spawn | `cli.ts:24` (`spawn`), `:25` (`join`), `:28` (`REPO_ROOT`) | dead once the spawn is removed (verified: no other use in `cli.ts`) |

The file-top docstring (`cli.ts:9-15`), the `DEFAULT_ENGINE` docstring
(`cli.ts:41-47`), and `CLI_USAGE` (`cli.ts:182-188`) all still describe `py` as a
working delegation (“delegate to the unchanged Python pipeline… spawn
`python3 adw/issue.py` … with the FULL parent env”). In this port that contract
is unsatisfiable.

### 2.2 Why bundling Python is the wrong direction (rules option 1 out)

The issue offers two acceptance paths. The repository’s own design record points
decisively away from “bundle the Python sibling”:

- **The port is deliberately self-contained.** `HEALTHTECH_PORT.md:20-21`: “This
  port is **TypeScript-only and self-contained**.” `HEALTHTECH_PORT.md:25` already
  documents the intended end-state: default `ts`, “the Python sibling is not
  bundled; `py` stays selectable but fails loudly.”
- **Coexistence is explicitly out of scope here.** `MVP-READINESS.md:121-124`:
  “**`ADW_ENGINE` py↔ts coexistence tested in the *integrated* repo.** The Python
  sibling is not bundled here, so **(C) cannot be validated from this standalone
  port** — it needs the combined environment.” The MVP is **claude + GitHub only**
  (`MVP-READINESS.md:136`).
- **`adw/` is a JSON-only contract surface, by design.** `HANDOVER.md:17-19`:
  cross-language state lives in `adw/state.schema.json` + `adw/fixtures/…`; “The
  Python sibling is not bundled in this standalone port.”

Bundling `adw/issue.py` would re-import the entire upstream Python pipeline plus a
`python3` runtime dependency, reintroduce a parity/maintenance burden the port was
created to shed, and contradict the MVP scope. **This spec rejects option 1.**

### 2.3 The error model to reuse

`AdwError` (`src/errors.ts:8-13`) is the control plane’s expected-failure type.
`main()` already catches it and renders `error: <message>` + rc `1`
(`cli.ts:460-466`), letting non-`AdwError` throws propagate as bugs. The fix needs
no new error class — just a new, specific `AdwError` message on the `py` branch.

---

## 3. Decision

**Recommended: Option 2 — fail closed with an explicit, dedicated `AdwError`,
and delete the dead spawn machinery.**

Selecting the `py` engine (via `--engine py` or `ADW_ENGINE=py`) must produce a
deterministic, descriptive `AdwError` at dispatch time — *before* any subprocess
is attempted — and the surrounding docs/help must describe `py` as unavailable in
this distribution rather than as a working delegation.

### 3.1 Keep `py` a *recognized* engine id (do not drop it from `ENGINE_IDS`)

`py` stays in `ENGINE_IDS` / `EngineId` and `resolveEngineId('py')` keeps
returning `'py'`. The unavailability is enforced one layer down, in `main()`’s
engine dispatch. Rationale:

1. **It satisfies the acceptance wording.** The criterion asks for an explicit
   “not available in this distribution” `AdwError`. Keeping `py` recognized lets
   us emit exactly that. Dropping `py` from `ENGINE_IDS` would instead yield the
   generic `unknown engine: 'py' (valid: ts)` — which reads as a typo, not as a
   deliberately-omitted-but-known mode, and does **not** match the requested
   message.
2. **It distinguishes “known-but-unavailable here” from “unknown/typo.”** An
   operator migrating from upstream who types `--engine py` deserves the real
   reason, not “unknown engine.”
3. **It preserves the two-engine conceptual model** that every doc and the
   cross-language contract assume, and keeps re-bundling a one-branch restore if a
   future integrated build ever wants it.

**Trade-off to accept (documented, not hidden):** the generic unknown-engine error
will still print `(valid: py, ts)` (`cli.ts:62`), advertising `py` as a valid
selector even though selecting it now errors. This is acceptable because `py`
*is* a valid selector that **reports its own unavailability**; the dedicated
message removes any ambiguity. (Alternative considered: rewrite the valid-list to
annotate `py` as unavailable — judged not worth the churn. See §8.)

### 3.2 Throw at the dispatch layer, not inside `resolveEngineId`

`resolveEngineId` is a pure validator also used to resolve the default; it must
not carry the “py is unavailable here” *policy*. The throw belongs in `main()`’s
engine-routing branch, replacing the spawn.

### 3.3 Canonical error message

```
the 'py' engine is not available in this standalone distribution: it requires the
Python sibling (adw/issue.py), which is not bundled here. Use --engine ts (the
default), or run the py engine from the integrated upstream repository.
```

Single line in code (no embedded newlines). The exact prose may be tightened in
review; the **load-bearing substring** that tests pin is
`not available in this standalone distribution`.

---

## 4. Implementation steps

> Production code under `adw_sdlc/src/` and tests under `adw_sdlc/test/`. **Do not
> implement in this planning phase.**

### Step 1 — Replace the `py` dispatch branch with a fail-closed throw

In `cli.ts:432-435`, change:

```ts
if (engine === 'py') {
  const forwarded = passthru.length > 0 ? [...rest, '--', ...passthru] : rest;
  return await deps.runPyEngine(forwarded);
}
```

to a dedicated throw, e.g.:

```ts
if (engine === 'py') {
  throw new AdwError(PY_ENGINE_UNAVAILABLE);
}
```

where `PY_ENGINE_UNAVAILABLE` is a module-level `const` holding the §3.3 message.
The `AdwError` is caught by the existing handler (`cli.ts:460-466`) → `error: …`
+ rc 1. The `rest`/`passthru` plumbing is no longer consulted on the py path
(both remain in use by the ts path: `extractEngineFlag` still strips `--engine`,
and `splitPassthru` still feeds the ts-path passthru rejection at
`cli.ts:437-443`).

### Step 2 — Delete the dead spawn machinery

- Remove `spawnPyEngine` (`cli.ts:369-385`).
- Remove `runPyEngine` from the `CliDeps` interface (`cli.ts:356-358`) and from
  `defaultCliDeps()` (`cli.ts:390`). (Conservative alternative in §8 keeps the
  seam; the recommendation is full removal — a non-functional injectable seam is
  exactly the “non-functional path” this issue exists to retire.)
- Remove the now-unused imports: `spawn` (`cli.ts:24`), `join` (`cli.ts:25`),
  `REPO_ROOT` (`cli.ts:28`). Confirmed: each is used **only** by `spawnPyEngine`.
  Typecheck (`noUnusedLocals`) will catch a miss.

### Step 3 — Correct the docstrings and help so `py` is never described as working

- File-top docstring (`cli.ts:9-15`): replace the “delegate to the unchanged
  Python pipeline / spawn `python3 adw/issue.py` … FULL parent env” paragraph with
  a short statement that `py` is **not available in this standalone port** and
  raises an explicit `AdwError`; keep the `ts` paragraph.
- `DEFAULT_ENGINE` docstring (`cli.ts:41-47`): change “fail loudly unless that
  sibling is added” to reflect the deterministic `AdwError` (no spawn, no
  `python3` dependency).
- `CLI_USAGE` (`cli.ts:182-188`): change the `--engine py delegates to a python3
  adw/issue.py sibling, which is NOT bundled…` clause to e.g. “`--engine py` is
  **not available in this distribution** (Python sibling not bundled); use
  `--engine ts` (the default).” Keep the rest of the usage block.

### Step 4 — Rewrite the spawn-pinning test file

`test/cli-py-engine.test.ts` currently pins the **real** `python3 adw/issue.py`
spawn over a mocked `node:child_process` (docstring + 3 tests, all asserting spawn
shape/rc-mapping). Under the decision there is no spawn. Rewrite the file
(keep the filename — it still documents the py-engine path) so it pins the new
contract over the same `spawnMock`:

1. `--engine py` → `main(['5','--yes'], { env:{} })` returns **rc 1**, stderr
   contains `not available in this standalone distribution`, **and `spawnMock` was
   never called** (the strongest regression pin: we must not even attempt a
   subprocess).
2. `ADW_ENGINE=py` env (no flag) → same outcome.
3. Extra args / post-`--` passthru present (`--engine py 5 --runner gemini -- --x`)
   → still fails closed; argv content is irrelevant on a dead path; `spawnMock`
   never called.

### Step 5 — Update `test/cli.test.ts`

- `cliDeps` helper (`cli.test.ts:33`): drop the `runPyEngine: vi.fn(async () => 0)`
  field once `CliDeps.runPyEngine` is removed.
- `describe('resolveEngineId')` (`cli.test.ts:45-62`): unchanged under the
  recommendation — `resolveEngineId('py')` still returns `'py'` (`:53-56`), and the
  ts-default assertions (`:46-51`) still hold.
- `describe('main — engine dispatch')`:
  - Replace **“delegates to the py engine when selected, forwarding argv verbatim”**
    (`cli.test.ts:268-279`) with a test asserting `--engine py` returns rc 1, prints
    the unavailable message, and never touches `loadRunner`/`runIssue`.
  - Replace/remove **“strips --engine from the argv forwarded to the py engine”**
    (`cli.test.ts:281-287`) — there is no forwarding anymore; `--engine` stripping
    is already covered by the `extractEngineFlag` unit tests (`cli.test.ts:73-89`).
  - Add a case: `ADW_ENGINE=py` from the environment fails closed identically.
- `describe('CLI usage')` (`cli.test.ts:93-99`): update any assertion that pins the
  old “delegates to a python3 adw/issue.py sibling” wording to the new
  “not available” wording (verify the exact assertion when implementing).

### Step 6 — Reconcile operator-facing docs

- `HEALTHTECH_PORT.md:25`: change “`py` stays selectable but fails loudly” →
  `py` is rejected with an explicit “not available in this distribution” error
  (no `python3` dependency, no missing-file spawn).
- `MVP-READINESS.md:121-124`: optional one-line pointer that, in the standalone
  port, the `py` path now **fails closed explicitly** (the coexistence gate itself
  stays ❌/post-MVP — unchanged).
- `HANDOVER.md:17-19`: already accurate (“Python sibling is not bundled”); add a
  short note that selecting `py` raises an explicit `AdwError`.
- `PLAN.md` step 10/12 (`PLAN.md:979-986`, live-smoke examples `:1003-1005`)
  describe the upstream delegation behavior. This is a **historical design
  record**; see Open Questions (§9) for whether to touch it. Default
  recommendation: a light-touch one-line callout near the engine-selection
  section pointing at this decision, leaving the historical narrative intact.

### Step 7 — Verify

From `adw_sdlc/`:

```bash
npm test -- cli.test.ts cli-py-engine.test.ts   # focused first
npm run verify                                   # full gate before reporting
```

`.adw/prompts` needs **no** regeneration: the only `.adw/prompts` hit for
“engine” is the word “engineer” in `plan.md:85` (verified), so the prompt-pack
drift check is unaffected.

---

## 5. Acceptance criteria

Maps the issue’s two checkboxes to this decision:

- [ ] **Decision implemented (option 2).** `--engine py` and `ADW_ENGINE=py` both
  produce a deterministic `AdwError` whose message contains `not available in this
  standalone distribution`, rendered as `error: …` with **rc 1** — with **no
  subprocess spawned** and **no dependency on `python3`**.
- [ ] **Tested behavior matches the decision.**
  - `test/cli-py-engine.test.ts` pins that `spawn` is never called for `py` (flag
    and env), rc 1, and the message substring.
  - `test/cli.test.ts` engine-dispatch + usage tests updated; no remaining
    assertion expects a `py` delegation/forward.
- [ ] **No dead code remains.** `spawnPyEngine`, `CliDeps.runPyEngine`, and the
  `spawn`/`join`/`REPO_ROOT` imports are gone; typecheck is clean.
- [ ] **Docs are honest.** `cli.ts` docstrings, `CLI_USAGE`, `HEALTHTECH_PORT.md`,
  and `HANDOVER.md` no longer describe `py` as a working delegation.
- [ ] **`npm run verify` is green** from `adw_sdlc/`.

---

## 6. Test strategy

- **Unit (primary):** `cli.test.ts` (engine resolution + `main()` dispatch) and the
  rewritten `cli-py-engine.test.ts` (spawn-never-called pin). Both mock seams; no
  network, no real subprocess — consistent with the suite’s “mock the seam, not the
  SDK” discipline.
- **Negative pins that must stay green:** unknown engine still
  `error: unknown engine: 'rust' (valid: py, ts)` rc 1 (`cli.test.ts:325-331`);
  the ts default path is untouched (`cli.test.ts:259-266`).
- **Boundary regression guard:** the new spawn-never-called assertion replaces the
  old “FULL parent env, no `env:` option” pin (`cli-py-engine.test.ts:50-57`) —
  there is no longer a child process, so the relevant invariant is simply *no
  spawn at all*, which is strictly stronger for the secret boundary.

---

## 7. Risks & mitigations

| Risk | Likelihood | Mitigation |
| --- | --- | --- |
| Removing `CliDeps.runPyEngine` breaks an external importer of the `CliDeps` type | Low | `CliDeps` is an internal CLI seam (test injection); the conservative §8 variant keeps the field if a consumer is found. |
| A doc elsewhere still claims `py` delegates and silently rots | Medium | §4 Step 6 enumerates the doc sites; `grep -rn "issue.py\|--engine py\|delegate" adw_sdlc` after editing to confirm none describe a working delegation. |
| Re-bundling Python later is now harder | Low | The change is a single-branch + seam restore from git history; the two-engine model and `ENGINE_IDS` are preserved. |
| Operator scripts relying on the old exit code from the missing-file spawn | Very low | The old rc was non-deterministic (1 vs python’s 2); standardizing on rc 1 with a clear message is a strict improvement and is documented in the issue/PR. |

---

## 8. Alternatives considered (and why not)

- **Option 1 — bundle `adw/issue.py` + the Python pipeline.** Rejected: contradicts
  the self-contained port (`HEALTHTECH_PORT.md:20-21`), is explicitly out of scope
  for this distribution (`MVP-READINESS.md:121-124`), and adds a `python3` runtime
  dependency and parity burden.
- **Drop `py` from `ENGINE_IDS` entirely.** Rejected as the primary path: yields
  `unknown engine: 'py' (valid: ts)`, which does **not** match the acceptance
  wording (“explicit ‘not available in this distribution’ `AdwError`”) and reads as
  a typo rather than a deliberately-omitted mode. (Could be layered on later if the
  team prefers a smaller surface; see §9.)
- **Keep the `runPyEngine` seam but have `defaultCliDeps` inject a throwing stub.**
  Conservative variant that preserves the injection surface. Rejected as default
  (pointless indirection for a dead capability) but acceptable if a `CliDeps`
  consumer turns up.
- **Annotate the unknown-engine valid-list as `(valid: ts; py: not bundled)`.**
  Extra churn in a hot error string for marginal clarity; the dedicated py message
  already disambiguates. Deferred.

---

## 9. Open questions

1. **Should `py` remain in `ENGINE_IDS`/`EngineId`?** This spec recommends **yes**
   (keep it recognized; fail at dispatch) to satisfy the acceptance wording and
   preserve the two-engine model. Confirm with the maintainer; if they prefer the
   smallest surface, switch to dropping `py` and accept the generic
   `unknown engine` message instead.
2. **How far to edit `PLAN.md`?** It is an upstream-derived historical roadmap whose
   step 10/12 describe the delegation as the intended behavior. Default: a
   light-touch callout only. Confirm whether a deeper reconciliation is wanted.
3. **Exact error wording.** The load-bearing substring is
   `not available in this standalone distribution`; the remainder is open to
   review-time polish.
