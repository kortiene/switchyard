# Spec: Drift guard for `ADW_*` env naming in src (and optionally docs/prompts)

- **Issue:** #6 — `fix: drift guard for ADW_* env naming in docs/prompts`
- **Labels:** `issue_class:fix`, `adw-live-batch`
- **Planned ADW run mode:** native
- **Type:** test/lint-only guard (no production behavior change)
- **Primary new file:** `adw_sdlc/test/env-naming-drift.test.ts` (vitest)
- **Source of truth referenced:** `adw_sdlc/src/env-vars.ts` (`ENV_ALIASES`, `modelEnvAlias`)
- **Runs under:** `npm test` → `npm run verify`

---

## 1. Context & current state (read this first)

A recent migration renamed the control-plane env knobs from `MX_AGENT_*` to
canonical `ADW_*`, keeping `MX_AGENT_*` as **deprecated compatibility aliases**.
The alias machinery lives entirely in `adw_sdlc/src/env-vars.ts`:

- `ENV_ALIASES` — the 7 canonical/legacy pairs (`ADW_ENGINE`/`MX_AGENT_ENGINE`,
  `ADW_RUNNER`/`MX_AGENT_RUNNER`, `ADW_TEST_CMD`/`MX_AGENT_TEST_CMD`,
  `ADW_FINALIZE_GATES`/`MX_AGENT_FINALIZE_GATES`,
  `ADW_CLASSIFY_ON_RUNNER`/`MX_AGENT_CLASSIFY_ON_RUNNER`,
  `ADW_ASSUME_YES`/`MX_AGENT_YES`,
  `ADW_PARITY_FORCE_FENCED_JSON`/`MX_AGENT_FORCE_FENCED`).
- `modelEnvAlias(phase)` — per-phase `ADW_MODEL_<PHASE>` / `MX_AGENT_MODEL_<PHASE>`.
- `readEnvAlias` / `readEnvFlag` — the only sanctioned readers. They prefer the
  canonical name, accept the legacy alias with a one-time stderr deprecation
  warning, and **throw** if canonical and legacy disagree.

**Crucial finding — there are zero bare `MX_AGENT_*` env reads in `src/` today.**
A repo-wide scan shows every control-plane env read already routes through the
`env-vars.ts` helpers (`adw_sdlc/src/cli.ts`, `orchestrator.ts`, `exec.ts`,
`models.ts`). The literal token `MX_AGENT_` appears in `src/` only in these
**legitimate, non-read** forms, which the guard must NOT flag:

| Location | Form | Why it's legitimate |
|---|---|---|
| `src/env-vars.ts` | `legacy: 'MX_AGENT_ENGINE'`, … | The sanctioned home of the alias table (string-literal values). |
| `src/env.ts:39` | `ENV_DENY_PREFIXES = ['MATRIX_', 'MX_AGENT_', 'ADW_']` | Deny-prefix constant; element is the bare prefix `'MX_AGENT_'` (no suffix). |
| `src/cli.ts:186,191,202` | help text `… (deprecated alias: MX_AGENT_RUNNER)` | Operator docs in `--help`, explicitly marked deprecated. |
| `src/runners/runner-mock.ts:6`, `src/orchestrator.ts:95` | comments `…MX_AGENT_-prefixed…` | Prose comments, hyphen/`*` after the prefix. |

So the guard is a pure **regression preventer**: it must fail *only* if someone
later introduces a new bare read of an `MX_AGENT_*` key outside `env-vars.ts`,
while leaving today's tree green.

### What "bare read" means precisely

A bare read is an expression that indexes an env-like object by an
`MX_AGENT_*`-suffixed key. The two syntactic shapes to forbid:

1. **Bracket index:** `process.env['MX_AGENT_RUNNER']`, `env["MX_AGENT_FOO"]`,
   `source['MX_AGENT_X']`, `deps.env['MX_AGENT_Y']`.
2. **Dot member:** `process.env.MX_AGENT_RUNNER`.

The discriminator that cleanly separates these from every legitimate mention in
the table above is: **a real read always has an uppercase letter immediately
after the `MX_AGENT_` prefix** (`MX_AGENT_RUNNER`, `MX_AGENT_MODEL_IMPLEMENT`,
…), whereas the deny-prefix constant is the bare `'MX_AGENT_'` (suffix = closing
quote) and the comments write `MX_AGENT_-prefixed` / `MX_AGENT_*` (suffix = `-`
or `*`). Requiring `MX_AGENT_[A-Z]` plus the bracket/dot access shape excludes
all current legitimate mentions without an allowlist.

### Repository conventions this spec mirrors

- `test/env-vars.test.ts` — the existing alias unit tests (style for env tests).
- `test/mvp-readiness-doc.test.ts`, `test/observed-live-ledger-doc.test.ts` —
  precedent for tests that `readFileSync` committed files and assert on content,
  resolving paths via `REPO_ROOT` from `../src/common.js`.
- `scripts/check-adw-sdlc-env.sh` (wired to `npm run lint:env`) — the existing
  **static source-level** secret-boundary lint (forbids `...process.env`
  spreads and banned opencode imports). Conceptually adjacent but about *secret
  leakage*, not *naming drift* (see §3.3 for why this guard is separate).
- The parity-rate spec's "core purity guard" (read the source, regex-assert it
  stays clean) is the same shape of guard proposed here.

---

## 2. Goal & acceptance criteria

### Goal
Prevent silent regression of the env rename: guarantee that `src/` reads
control-plane env **only** through the `env-vars.ts` alias helpers, so no new
bare `MX_AGENT_*` read can be reintroduced as if it were canonical — while
keeping `MX_AGENT_*` fully working as deprecated aliases.

### Acceptance criteria (from the issue)
- [ ] The guard **fails** if a new bare `MX_AGENT_*` read is added to `src/`
      outside `env-vars.ts`.
- [ ] Existing canonical `ADW_*` usage **passes** (tree is green today).
- [ ] `npm run verify` stays green.

### Additional acceptance criteria (this spec)
- [ ] The guard scans every `src/**/*.ts` file except `env-vars.ts` and reports
      offending `file:line` on failure.
- [ ] The guard includes a **self-test**: it asserts the detector *does* fire on
      a synthetic positive (`process.env['MX_AGENT_RUNNER']`) and *does not* fire
      on known-good strings (the deny-prefix array, the cli help text, a
      canonical `readEnvAlias(env, ENV_ALIASES.runner)` call). This proves the
      guard isn't trivially-true.
- [ ] No production code under `src/`, no prompt-pack (`.adw/`) changes, so
      `pack:check` is unaffected.

---

## 3. Recommended design

**Primary: a vitest guard at `test/env-naming-drift.test.ts`.** (The issue
permits "script or vitest".)

### 3.1 Why vitest over extending the bash lint

- It can import the typed `ENV_ALIASES` / `modelEnvAlias` as the **single source
  of truth** for the legacy names, instead of hard-coding `MX_AGENT_` in a
  second place.
- It can **self-test** the detector (positive + negative fixtures), so the guard
  can't silently rot into a no-op regex.
- Better failure messages (`file:line` + the offending text) than `grep`.
- It runs inside `npm test` (already in `verify`), and `tsc --noEmit` typechecks
  `test/` (`tsconfig.json` `include` = `["src","test","tools"]`).
- Keeps *naming-drift* separate from the *secret-leakage* concern that
  `check-adw-sdlc-env.sh` owns (see §3.3). A bash alternative is fully specified
  in §7 for implementers who prefer co-locating static checks.

### 3.2 The detector

Two regexes, both requiring an uppercase letter after the prefix so the
deny-prefix constant and prose comments are excluded by construction:

```ts
// Bracket index: env['MX_AGENT_RUNNER'] / process.env["MX_AGENT_FOO"]
const BRACKET = /\[\s*['"`]MX_AGENT_[A-Z][A-Z0-9_]*['"`]\s*\]/;
// Dot member:  process.env.MX_AGENT_RUNNER
const DOT = /\.\s*MX_AGENT_[A-Z][A-Z0-9_]*/;
```

A line is a violation iff it matches `BRACKET` **or** `DOT`. Verified against the
current tree, these match **none** of the legitimate mentions in §1:
`'MX_AGENT_'` (deny prefix, no suffix letter), `(deprecated alias: MX_AGENT_RUNNER)`
(preceded by space, not `.`/`[`), `MX_AGENT_-prefixed` / `MX_AGENT_*` (suffix is
`-`/`*`), and the `legacy: 'MX_AGENT_ENGINE'` object values (preceded by `: `,
and in the excluded file anyway).

> **Comment caveat:** a `//` or `/* */` comment that literally contains
> `env['MX_AGENT_X']` *would* trip the detector. That is itself drift-y example
> code and should be written canonically, but to avoid the false positive the
> implementer SHOULD strip `//`-to-end-of-line comments before matching (cheap;
> see §5 step 3). Block-comment stripping is optional; none exist in `src/` that
> would matter today. Document whichever choice is made in the test header.

### 3.3 Why this is separate from `check-adw-sdlc-env.sh`

`check-adw-sdlc-env.sh` enforces the **secret boundary** (no `...process.env`
spread; only `createOpencodeClient`; opencode `/v2/client` subpath). That gate is
about *what reaches a runner child*. This guard is about *naming hygiene of the
migration* — orthogonal. Keeping them in different files keeps each gate's intent
legible. (If the maintainer prefers one place for all static source checks, §7
adds the equivalent as Check 4 in the bash script instead — pick one, not both.)

### 3.4 File enumeration

Walk `src/` recursively, filter to `*.ts`, exclude `env-vars.ts`:

```ts
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { REPO_ROOT } from '../src/common.js';

const SRC_DIR = join(REPO_ROOT, 'adw_sdlc', 'src');
// Node >= 20.19 (package.json engines) supports recursive readdirSync.
const files = readdirSync(SRC_DIR, { recursive: true, encoding: 'utf8' })
  .filter((p) => p.endsWith('.ts'))
  .filter((p) => p !== 'env-vars.ts' && !p.endsWith(`${'/'}env-vars.ts`));
```

(Use a small explicit recursive walk if the maintainer prefers not to rely on
`recursive: true`; either is fine. `src/` has one subdir, `runners/`.)

---

## 4. Test plan (`test/env-naming-drift.test.ts`)

1. **No bare `MX_AGENT_*` reads in `src/` (the guard).** For each scanned file,
   strip `//` comments per line, test each line against `BRACKET`/`DOT`, collect
   `{ file: relative(REPO_ROOT, path), line, text }` violations. Assert the
   collected list is empty; on failure, the assertion message lists every
   `file:line: <text>`. **(Issue AC #1 + #2 — green today, fails on regression.)**

2. **Detector self-test — positives.** Assert `BRACKET`/`DOT` match each of:
   `process.env['MX_AGENT_RUNNER']`, `env["MX_AGENT_FOO"]`,
   `process.env.MX_AGENT_RUNNER`, `source['MX_AGENT_MODEL_IMPLEMENT']`. Proves
   the guard actually fires.

3. **Detector self-test — negatives (known-good).** Assert neither regex matches:
   - `['MATRIX_', 'MX_AGENT_', 'ADW_']` (deny-prefix constant),
   - `Env: ADW_RUNNER (deprecated alias: MX_AGENT_RUNNER)` (cli help text),
   - `MATRIX_-/ADW_-/MX_AGENT_-prefixed key` (comment prose),
   - `readEnvAlias(env, ENV_ALIASES.runner)` and `process.env['ADW_RUNNER']`
     (canonical usage). **(Issue AC #2 — canonical passes; guards against
     false positives.)**

4. **Source-of-truth coverage (cheap consistency check).** For every legacy name
   derived from `ENV_ALIASES` (`Object.values(ENV_ALIASES).map(a => a.legacy)`)
   plus a sample `modelEnvAlias('implement').legacy`, assert the bare-read form
   `env['<legacy>']` is caught by the detector. This ties the guard to the alias
   table so adding a new alias automatically extends the guard's intent.

5. **(Optional) docs/prompts advisory scan** — see §6; recommend deferring or
   shipping behind the lenient heuristic there. Not required for any AC.

All assertions use the existing `vitest` `describe/it/expect` style (mirror
`test/env-vars.test.ts`). Mind the strict tsconfig: `noUncheckedIndexedAccess`
means guard array-index access (e.g. capture groups, `lines[i]`).

---

## 5. Step-by-step implementation

1. Create `adw_sdlc/test/env-naming-drift.test.ts`. Header comment: state that
   this is the env-rename drift guard (canonical `ADW_*`, deprecated
   `MX_AGENT_*` aliases), that `env-vars.ts` is the sanctioned home, and the
   comment-stripping choice.
2. Define `BRACKET` and `DOT` (§3.2) and a `findViolations(text): {line,text}[]`
   helper that splits on `\n`, strips `//` comments, and tests each line.
3. Comment stripping: for each line, cut at the first `//` **not inside a
   string** — a pragmatic version is to drop from the first `//` to EOL; this is
   acceptable because no legitimate `src/` line has `//` inside a string before
   an `MX_AGENT_[A-Z]` read. Keep it simple and documented.
4. Enumerate `src/**/*.ts` minus `env-vars.ts` (§3.4); read each; collect
   violations with relative paths.
5. Write tests 1–4 from §4. For test 1's failure message, `join` the violations
   into a readable multi-line string.
6. Run focused: `npx vitest run test/env-naming-drift.test.ts` from `adw_sdlc/`.
   Expect green (no current violations) and the self-tests passing.
7. Sanity-check the guard *can* fail: temporarily add
   `const x = process.env['MX_AGENT_RUNNER'];` to a throwaway spot in, say,
   `src/exec.ts`, re-run the focused test, confirm it **fails** with that
   `file:line`, then revert. (Do not commit the temporary edit.)
8. Run the full gate: `npm run verify` from `adw_sdlc/`.

No production `src/` changes. No `.adw/` changes (so `pack:generate` /
`pack:check` are untouched).

---

## 6. Optional docs/prompts scan (the issue's "optionally")

The issue optionally asks to "scan committed docs/prompts for bare `MX_AGENT_*`
not marked as a deprecated alias." This is genuinely fuzzier than the `src/`
guard and carries false-positive risk:

- Every current doc mention is already marked (`README.md`: "legacy
  `MX_AGENT_*`"; project-context: "deprecated `MX_AGENT_*` compatibility
  aliases"; `HANDOVER.md`, `PARITY.md`, `cli.ts --help`). So a marker-based
  heuristic passes today.
- `.adw/prompts/*.md` are **generated** from `.adw/pack.profile.json`
  (`npm run pack:generate`, guarded by `pack:check`). Scanning generated outputs
  duplicates a concern whose real source is the profile; prefer scanning the
  **profile** if anything.

**Heuristic (if implemented):** a line containing an `MX_AGENT_[A-Z]` token must
also contain, on the same line, one of: `deprecat`, `alias`, `legacy`,
`compat`, `inherited`, or be the literal glob `MX_AGENT_*`. Scan a small,
explicit allowlist of paths to bound scope, e.g.: `adw_sdlc/README.md`,
`adw_sdlc/HANDOVER.md`, `adw_sdlc/PARITY.md`, `adw_sdlc/MVP-READINESS.md`,
`.adw/pack.profile.json`. Treat hits as failures with `file:line`.

**Recommendation:** ship the `src/` guard (§3–§5) as the required deliverable;
implement the docs scan only if the maintainer wants it, behind the lenient
heuristic above, as a clearly-separated `describe('docs/prompts MX_AGENT_*
marking')` block (or a second file `test/env-naming-docs-drift.test.ts`). Default
to **deferring** the prompts portion (covered indirectly by `pack:check`) and, if
anything, guarding `.adw/pack.profile.json` + the top-level docs. See open
question O-3.

---

## 7. Alternative implementation (bash, as Check 4 in the existing lint)

If the maintainer prefers all static source checks in one place, add to
`scripts/check-adw-sdlc-env.sh` (after Check 3), instead of the vitest file:

```bash
# --- Check 4: no bare MX_AGENT_* env read outside env-vars.ts -----------------
# Forbid bracket/dot access to an MX_AGENT_*<suffix> key anywhere in src except
# env-vars.ts (the sanctioned alias home). Requires a letter after the prefix so
# the ENV_DENY_PREFIXES constant ('MX_AGENT_') and prose ('MX_AGENT_*') are not
# flagged.
mx_hits="$(grep -rnE "(\[[[:space:]]*['\"]MX_AGENT_[A-Z][A-Z0-9_]*['\"][[:space:]]*\]|\.[[:space:]]*MX_AGENT_[A-Z][A-Z0-9_]*)" \
  "${SRC_DIR}" --exclude='env-vars.ts' || true)"
if [[ -n "${mx_hits}" ]]; then
  report "bare MX_AGENT_* env read outside env-vars.ts (read via env-vars.ts alias helpers instead)" "${mx_hits}"
fi
```

Trade-offs: runs in `lint:env` (also in `verify`), single place for static
checks, **but** can't self-test, hard-codes the prefix, and can't reference
`ENV_ALIASES`. Pick **either** vitest (§3, recommended) **or** this — not both.

---

## 8. Verification

- Focused: `npx vitest run test/env-naming-drift.test.ts` (from `adw_sdlc/`).
- Negative proof (manual, then revert): temporarily add a bare
  `process.env['MX_AGENT_RUNNER']` read to a `src/` file and confirm the guard
  fails with that `file:line` (§5 step 7).
- Full gate: `npm run verify` from `adw_sdlc/`
  (`typecheck → lint:env → pack:check → test → build && rm -rf dist`). Must stay
  green. `typecheck` covers the new test (`tsconfig.json` includes `test`);
  `build` is `src`-only so the test never ships to `dist`; `pack:check` is
  unaffected (no `.adw/` edits).

---

## 9. Docs to check (small or none)

- `adw_sdlc/HANDOVER.md` — if the repo convention is to log new guards/tests,
  add a one-line note that `test/env-naming-drift.test.ts` guards the
  `ADW_*`/`MX_AGENT_*` rename (and bump any "test count" line the repo keeps —
  recent commits maintain such a count). Keep additive.
- `adw_sdlc/docs/LIVE-RUN-BATCH.md` — issue #6 row already describes this work;
  no change needed unless marking it run.
- No `README.md`/`PARITY.md`/`MVP-READINESS.md` content change required; the
  guard documents itself.

---

## 10. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Guard false-positives on legitimate mentions (deny prefix, help text, comments) | Detector requires `MX_AGENT_[A-Z]` **plus** bracket/dot access shape; negative self-test (§4 test 3) pins the known-good strings. Verified against current tree. |
| Guard is trivially-true (regex never matches) | Positive self-test (§4 test 2) + source-of-truth coverage (§4 test 4) prove it fires; §5 step 7 manual negative proof. |
| Indirection evades the regex (`const k='MX_AGENT_X'; env[k]`) | Accepted limitation of a "lightweight" guard; the realistic regression is a direct read. Documented here. |
| A `//`-comment example containing a read trips the guard | Strip `//`-to-EOL before matching (§5 step 3); document the choice. |
| Reliance on `readdirSync({recursive:true})` | Supported on Node ≥ 20.19 (package `engines`); fallback to a tiny manual walker if desired (§3.4). |
| Strict tsconfig (`noUncheckedIndexedAccess`, `noUnusedLocals`) breaks the test build | Guard array/capture-group access; avoid unused locals/params. Run `npm run typecheck`. |
| Scope creep into docs/prompts causing churn/false-positives | Make the docs scan optional and lenient (§6); default to deferring the generated-prompts portion (covered by `pack:check`). |
| Two overlapping guards (vitest + bash Check 4) double-maintained | Choose exactly one (§7). Recommendation: vitest. |

---

## 11. Out of scope

- Removing or changing `MX_AGENT_*` alias support — they must keep working
  (issue note). This guard only prevents *reintroducing them as canonical*.
- Any change to `env-vars.ts` semantics, `safeSubprocessEnv`, or the secret
  boundary (`check-adw-sdlc-env.sh` Checks 1–3).
- Any `src/` production code change, new CLI flags, or output changes.
- Regenerating `.adw/prompts` or editing `.adw/pack.profile.json` (unless the
  optional §6 docs scan is implemented against the profile).
