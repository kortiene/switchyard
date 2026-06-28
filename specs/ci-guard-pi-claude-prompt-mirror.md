# Spec: CI-guard the `.pi/prompts` ↔ `.claude/commands` byte-identical mirror

- **Issue:** #39 — `CI-guard the .pi/prompts ↔ .claude/commands byte-identical mirror`
- **Labels:** `issue_class:test`, `backlog`, `area:ci`
- **Type:** tooling + test hardening (no runtime/`src/` behavior change)
- **Owning files (new):** `adw_sdlc/tools/mirror.ts` (pure diff helper), `adw_sdlc/tools/mirror-check.ts` (CLI)
- **Owning files (edited):** `adw_sdlc/package.json` (scripts), `adw_sdlc/test/phases.test.ts`, `adw_sdlc/test/scaffold.test.ts`, `adw_sdlc/README.md`, `adw_sdlc/HANDOVER.md`, `.github/workflows/verify.yml` (comment), `adw_sdlc/src/pack-generator.ts` (comment)
- **Tests (new):** `adw_sdlc/test/mirror.test.ts`
- **Planned ADW run mode:** native

---

## 1. Context & current state (read this first)

The repo ships two parallel "neutral fallback command prompt" trees that must stay
**byte-for-byte identical**:

- `.pi/prompts/*.md` — Pi slash-command templates. This is the **canonical
  source of truth** (`adw_sdlc/src/pack-generator.ts:42`,
  `DEFAULT_TEMPLATES_DIR = '.pi/prompts'`). The project-pack generator renders
  these templates (plus `.adw/pack.profile.json`) into the runtime pack at
  `.adw/prompts/`.
- `.claude/commands/*.md` — Claude Code slash-command templates. These are a
  **hand-maintained mirror** of `.pi/prompts`. `pack-generator.ts` documents the
  relationship explicitly (lines 4–5): *"The neutral template prompts
  (`.pi/prompts/*.md`, mirrored byte-for-byte in `.claude/commands/*.md`) are the
  single source of truth."* `common.ts:7-9` and `phases.ts:5-6` restate the
  byte-identity invariant.

**Current state of each tree (verified):** 14 `.md` files in each root; the two
trees are byte-identical (`diff -rq .pi/prompts .claude/commands` ⇒ no output).
`classify.md` happens to also match `.adw/prompts/classify.md`; the other 13
`.adw/prompts` files differ because they carry the generated project-context
header — that is expected and out of scope here.

### What guards the mirror today, and the gap

| Guard | What it covers | Gap |
|---|---|---|
| `pack:check` (`tools/pack-generate.ts --check`) | `.adw/prompts` is a fresh render of `.pi/prompts` (drift guard). | Does **not** look at `.claude/commands` at all. `pack-generate.ts` only ever *reads* `.pi/prompts` and *writes* `.adw/prompts`. |
| `phases.test.ts:94` (single unit test) | `.pi/prompts` vs `.claude/commands`: same **`.md` basename set**, each `.md` byte-identical, and a project-neutrality regex. Runs inside `npm test` → `npm run verify`, so it *is* gate-wired. | (a) **`.md`-only** — a non-`.md` file (or a stray file) added to one root is invisible. (b) **Shallow** — `readdirSync` does not recurse; a future subdirectory drifts silently. (c) **One-directional file walk** conflated with a neutrality regex; low discoverability (buried in a phase-catalog test file). (d) No **standalone command** to reproduce/fix; the mirror remains "hand-maintained" with no sync affordance. |

So a gate-enforced byte-identity assertion technically already exists, but it is
narrow (`.md`-only, non-recursive) and there is no command to *repair* drift — the
mirror is maintained by hand, which is exactly the silent-drift risk the issue
calls out (evidence: `phases.test.ts:94`, `pack-generate.ts (only writes .adw)`).

### What "done" should mean

The issue's single acceptance bullet — *"A gate-enforced check asserts
`.pi/prompts` and `.claude/commands` are byte-identical"* — is best satisfied by
matching the **proven `pack:check` / `pack:generate` pattern**:

1. a recursive, **all-file** (not just `.md`), bidirectional byte-identity check,
2. surfaced as a first-class `npm run` script and wired into `npm run verify`
   (so it runs in the same gate the ADW orchestrator and CI already run), **and**
3. a `--write` / sync affordance that copies the canonical `.pi/prompts` tree
   onto `.claude/commands`, so the mirror stops being hand-maintained.

This both **hardens** the existing test and **closes the structural gap**
(`pack-generate.ts` writes `.adw` but nothing writes/repairs `.claude/commands`).

---

## 2. Goal & acceptance criteria

### Goal
Make the `.pi/prompts` ↔ `.claude/commands` byte-identical mirror a
**gate-enforced, recursive, all-file** invariant with a one-command repair path,
parallel to how `pack:check` / `pack:generate` guard `.adw/prompts`.

### Acceptance criteria (from the issue)
- [ ] A gate-enforced check asserts `.pi/prompts` and `.claude/commands` are
      byte-identical (it runs as part of `npm run verify`, which CI
      [`verify.yml`] runs as a required status check).

### Additional acceptance criteria (this spec)
- [ ] The check is **recursive** and compares **all regular files** (not only
      `.md`): a difference in file set (missing/extra) **or** in bytes of any
      common file fails the check.
- [ ] The check is **bidirectional**: a file present in one root but not the
      other fails, regardless of which root it is in.
- [ ] A standalone command (`npm run mirror:check`) reports drift and exits
      non-zero on any difference, writing nothing.
- [ ] A repair command (`npm run mirror:sync`) makes `.claude/commands`
      byte-identical to the canonical `.pi/prompts`, and is idempotent
      (re-running is a no-op).
- [ ] `npm run verify` runs `mirror:check` and stays green on the current
      (already-identical) trees.
- [ ] The pure diff logic is unit-tested over temp dirs (detects missing, extra,
      drifted, nested, and non-`.md` differences) without touching the repo trees.
- [ ] No change to `src/` runtime behavior, to prompt content, or to `.adw/`
      generated output. `pack:check` output is unaffected.

---

## 3. Recommended design (primary)

Mirror the pack-tooling shape: a **pure helper** + a **thin CLI**, plus a
hardened test. Place both new files under `tools/` (build-time tooling), matching
the `tools/parity-rate-core.ts` precedent so they are **typechecked** (`tsconfig.json`
includes `tools/`) but **never built to `dist`** (`tsconfig.build.json` includes
only `src/`).

```
adw_sdlc/
  tools/
    mirror.ts          ← NEW. Pure-ish diff helper (reads fs, no process/exit).
    mirror-check.ts    ← NEW. CLI: --check (default) / --write. No git, no network.
  test/
    mirror.test.ts     ← NEW. Unit tests for the helper over temp dirs + real-tree guard.
    phases.test.ts     ← EDIT. Route byte-identity through the helper; keep neutrality regex.
    scaffold.test.ts   ← EDIT. Extend the verify-chain assertions for mirror:check.
  package.json         ← EDIT. Add mirror:check / mirror:sync; insert mirror:check into verify.
```

### 3.1 `tools/mirror.ts` — the pure diff helper

A small module that diffs two directory trees recursively and returns a
structured result. It reads the filesystem but does **not** touch `process`,
print, or exit — so it is unit-testable and reusable by both the CLI and the test.

```ts
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

import { resolveRepoPath } from '../src/config.js';
import { AdwError } from '../src/errors.js';

/** Canonical mirror pairs: source of truth → mirror that must match it byte-for-byte. */
export const MIRROR_PAIRS: ReadonlyArray<{ source: string; mirror: string }> = [
  { source: '.pi/prompts', mirror: '.claude/commands' },
];

export interface MirrorDiff {
  /** Repo-relative paths present in source but missing from the mirror. */
  missing: string[];
  /** Repo-relative paths present in the mirror but not in source. */
  extra: string[];
  /** Repo-relative paths present in both but whose bytes differ. */
  drifted: string[];
}

export interface MirrorResult extends MirrorDiff {
  ok: boolean;
  source: string; // resolved absolute source dir
  mirror: string; // resolved absolute mirror dir
}

/** Sorted repo-relative file paths under `dir` (recursive, regular files only, POSIX-separated). */
export function listFilesRecursive(dir: string): string[] {
  const out: string[] = [];
  const walk = (abs: string): void => {
    for (const entry of readdirSync(abs, { withFileTypes: true })) {
      const child = join(abs, entry.name);
      if (entry.isDirectory()) {
        walk(child);
      } else if (entry.isFile()) {
        out.push(relative(dir, child).split(sep).join('/'));
      } else {
        // Symlinks / sockets / fifos are not expected in a prompt tree; fail loud.
        throw new AdwError(`unexpected non-regular entry in mirror tree: ${child}`);
      }
    }
  };
  walk(dir);
  return out.sort();
}

/** Compare two trees byte-for-byte over the union of their files. Pure (fs read only). */
export function diffMirror(sourceDir: string, mirrorDir: string): MirrorResult {
  const source = resolveRepoPath(sourceDir);
  const mirror = resolveRepoPath(mirrorDir);
  const srcFiles = new Set(listFilesRecursive(source));
  const mirFiles = new Set(listFilesRecursive(mirror));

  const missing = [...srcFiles].filter((f) => !mirFiles.has(f)).sort();
  const extra = [...mirFiles].filter((f) => !srcFiles.has(f)).sort();
  const drifted = [...srcFiles]
    .filter((f) => mirFiles.has(f))
    .filter((f) => readFileSync(join(source, f)) /* Buffer */.compare(readFileSync(join(mirror, f))) !== 0)
    .sort();

  return { ok: missing.length === 0 && extra.length === 0 && drifted.length === 0, missing, extra, drifted, source, mirror };
}
```

Notes:
- **Buffer compare**, not string compare — true byte-identity (no encoding/EOL
  normalization surprises). The current trees are LF text; this stays correct if
  a binary or CRLF file is ever introduced.
- `resolveRepoPath` (from `src/config.ts`) makes the helper CWD-independent, the
  same way `pack-generator.ts` resolves its dirs.
- `MIRROR_PAIRS` is a list so a second mirror pair can be added later (e.g. a
  future `codex`/`opencode` command root) with zero CLI changes.

### 3.2 `tools/mirror-check.ts` — the CLI

A thin CLI in the exact shape of `tools/pack-generate.ts`: `--check` is the
deterministic, CI-safe default (writes nothing, exits non-zero on drift);
`--write` repairs the mirror by copying the canonical source over it.

```
usage: mirror-check [--check] [--write] [--dry-run] [-h|--help]

Check (default) or repair the .pi/prompts ↔ .claude/commands byte-identical mirror.
The canonical source is .pi/prompts; --write makes .claude/commands match it.

Flags:
  --check     verify the mirror is byte-identical; write nothing (default)
  --write     copy the canonical source tree onto the mirror to fix drift
  --dry-run   with --write, show what would change; write nothing
  -h, --help  show this help
```

Behavior:
- **`--check` (default):** for each pair in `MIRROR_PAIRS`, call `diffMirror`. If
  any pair has `missing`/`extra`/`drifted`, print each bucket to stderr with the
  fix hint `run: npm run mirror:sync`, and return exit code `1`. On a clean tree,
  print `mirror is byte-identical` and return `0`.
- **`--write`:** make the mirror match the source exactly — write/overwrite every
  source file into the mirror (recursively, creating dirs) and **delete** any
  `extra` mirror file/dir not present in source (so sync is exact, not additive).
  Idempotent: re-running writes nothing. Report `synced: N; unchanged: M; removed: K`.
- **`--dry-run`** (only meaningful with `--write`): report what would change,
  write nothing.
- Argument/usage errors return exit code `2` (matches `pack-generate.ts`); known
  `AdwError`s print `error: <msg>` and return `2`.
- Guard the auto-run with the same
  `if (process.argv[1] === fileURLToPath(import.meta.url))` idiom as
  `pack-generate.ts`, and export `main(argv)` for testability.

This CLI does **no git and no network** — it only reads/writes the two local
prompt trees, consistent with the ADW phase rule that the orchestrator owns all
git/gh.

### 3.3 `package.json` — scripts + verify wiring

Add two scripts and insert `mirror:check` into `verify` **immediately after
`pack:check`** (the two are sibling prompt-tree drift guards):

```jsonc
"scripts": {
  // ...
  "pack:check": "tsx tools/pack-generate.ts --check",
  "mirror:check": "tsx tools/mirror-check.ts --check",
  "mirror:sync": "tsx tools/mirror-check.ts --write",
  "verify": "npm run typecheck && npm run lint:env && npm run pack:check && npm run mirror:check && npm test && npm run build && rm -rf dist"
}
```

New canonical chain:
`typecheck → lint:env → pack:check → mirror:check → test → build → rm -rf dist`.

### 3.4 Tests

**New `test/mirror.test.ts` — helper unit tests + real-tree guard.**

1. **Pure helper over temp dirs** (use `mkdtempSync(join(tmpdir(), 'mirror-'))`,
   clean up in `afterEach`): build a source and mirror dir and assert `diffMirror`
   returns the right buckets for:
   - identical trees (incl. a nested subdir and a non-`.md` file) ⇒ `ok: true`,
     all buckets empty;
   - a file only in source ⇒ `missing` lists it, `ok: false`;
   - a file only in mirror ⇒ `extra` lists it, `ok: false`;
   - a common file with one differing byte ⇒ `drifted` lists it, `ok: false`;
   - a **non-`.md`** differing file and a **nested** differing file are both
     caught (these are the gaps in the old `.md`-only/shallow test);
   - paths are reported repo/relative, POSIX-separated, and sorted.
2. **`listFilesRecursive`** returns sorted, recursive, POSIX-separated relative
   paths and excludes directories.
3. **Real-tree byte-identity guard** (the gate-relevant assertion): call
   `diffMirror('.pi/prompts', '.claude/commands')` and assert
   `result.ok === true` with empty `missing`/`extra`/`drifted`. Also assert the
   source tree is non-empty (`listFilesRecursive('.pi/prompts').length > 0`) so a
   future "both roots emptied" mistake cannot pass vacuously.

**Edit `test/phases.test.ts`** (the existing single test, lines 94–108): keep the
test name/intent but route the byte-identity through the shared helper and keep
the project-neutrality regex (a *separate* invariant worth preserving):

```ts
it('keeps fallback command prompts neutral and byte-identical across runner roots', () => {
  const result = diffMirror('.pi/prompts', '.claude/commands');
  expect(result, 'pi/claude prompt mirror drifted').toMatchObject({ ok: true, missing: [], extra: [], drifted: [] });

  const projectSpecific = /HealthTech|PRD_HealthTech|zero-knowledge|AES-256|ARTCI|Côte|Ivoire|crypto-core|app-patient|app-medecin/i;
  for (const file of listFilesRecursive(resolveRepoPath('.pi/prompts'))) {
    expect(readFileSync(join(resolveRepoPath('.pi/prompts'), file), 'utf8')).not.toMatch(projectSpecific);
  }
});
```

(Importing `diffMirror`/`listFilesRecursive` from `../tools/mirror.js`. The
real-tree byte-identity now lives in both `mirror.test.ts` and here; that overlap
is intentional and cheap — alternatively, move the neutrality check into
`mirror.test.ts` and delete this block. See O-3.)

**Edit `test/scaffold.test.ts`** — it pins the `verify` chain and ordering
(`scaffold.test.ts:50-88`). Update so the suite stays green and actually guards
the new stage:
- Add `mirror:check` (and assert `npm test` still present) to the required-stages
  list at `scaffold.test.ts:52`.
- Add an ordering assertion in the canonical-order test (`:65-75`):
  `idx('pack:check') < idx('mirror:check')` and `idx('mirror:check') < idx('npm test')`.
- The "all `npm run <stage>` references point to defined scripts" test (`:79`)
  needs no change beyond `mirror:check` existing in `scripts` (it will).

### 3.5 Docs & comments to update

- `adw_sdlc/README.md:135` — chain comment
  `# typecheck → lint:env → pack:check → test → build → rm -rf dist` → insert
  `→ mirror:check`. Add a short bullet near `:109`/`:155` documenting
  `npm run mirror:check` (drift guard) and `npm run mirror:sync` (repair), naming
  `.pi/prompts` as canonical.
- `.github/workflows/verify.yml:5-6` and `:50` — the comment that lists the
  stage chain (`typecheck -> lint:env -> pack:check -> test -> build -> clean`)
  → add `mirror:check`. No job logic changes (CI just runs `npm run verify`).
- `adw_sdlc/src/pack-generator.ts:4-5` and `:41` — extend the "mirrored
  byte-for-byte in `.claude/commands`" note to point at `npm run mirror:check`
  as the enforcing guard (so the next reader knows the mirror is now gated, not
  just asserted by a unit test).
- `adw_sdlc/HANDOVER.md` — add a short entry (repo convention) noting the mirror
  is now gate-enforced via `mirror:check` in `verify`, with `mirror:sync` as the
  repair path; update the verify-chain description and the running test/file
  counts to whatever the suite reports after this change.

---

## 4. Step-by-step implementation

1. **Add `tools/mirror.ts`** (§3.1): `MIRROR_PAIRS`, `listFilesRecursive`,
   `diffMirror`, and the `MirrorDiff`/`MirrorResult` types. No `process`, no
   printing.
2. **Add `tools/mirror-check.ts`** (§3.2): `parseArgs`, `main(argv)`, the
   `--check`/`--write`/`--dry-run` behaviors, and the `import.meta.url` run guard.
   Reuse `AdwError`/`resolveRepoPath`; model structure on `tools/pack-generate.ts`.
3. **Edit `package.json`** (§3.3): add `mirror:check` + `mirror:sync`; insert
   `npm run mirror:check` after `pack:check` in `verify`.
4. **Add `test/mirror.test.ts`** (§3.4): helper unit tests over temp dirs +
   real-tree guard.
5. **Edit `test/phases.test.ts`** (§3.4): route byte-identity through `diffMirror`;
   keep the neutrality regex; recurse via `listFilesRecursive`.
6. **Edit `test/scaffold.test.ts`** (§3.4): extend required-stages and ordering
   assertions for `mirror:check`.
7. **Update docs/comments** (§3.5): README chain + new scripts, `verify.yml`
   comment, `pack-generator.ts` comment, HANDOVER entry + counts.
8. **Verify** (§7): focused tests first, then `npm run verify`. Confirm
   `mirror:check` passes on the current trees and `mirror:sync` is a no-op.

No `src/` runtime code changes. No prompt-content changes. No `.adw/` regeneration
(so `pack:check` stays green and `pack:generate` need not run).

---

## 5. Alternatives considered

- **A — One-line bash gate (`lint:mirror`).** A `scripts/check-prompt-mirror.sh`
  running `diff -rq .pi/prompts .claude/commands` (exit non-zero on any
  difference), wired into `verify` like `lint:env`. *Pros:* dead-simple, recursive
  and all-file by construction, bulletproof byte-identity. *Cons:* no repair
  affordance (mirror stays hand-maintained), not unit-testable, adds another shell
  dependency, and `diff` output is less actionable than structured buckets.
  **Rejected as primary** because it does not address the issue's second evidence
  point (nothing *writes* the mirror); kept as a viable lighter-weight option.
- **B — Harden the existing unit test only.** Make `phases.test.ts:94` recursive +
  all-file + bidirectional (via an inline helper), no new CLI/script. *Pros:*
  smallest diff; already gate-wired through `npm test`. *Cons:* keeps the mirror
  hand-maintained (no sync command), low discoverability, and offers no
  reproduce/fix command outside vitest. **Rejected as primary**; this is the
  fallback if the maintainer wants minimum surface (still satisfies the single
  issue AC).
- **C — Generate `.claude/commands` from `.pi/prompts` in `pack-generate.ts`.**
  Fold a mirror-copy step into the existing generator. *Pros:* one tool. *Cons:*
  overloads the pack generator (whose job is template→pack rendering, not 1:1
  tree mirroring) and couples two distinct concerns; harder to reason about
  `pack:check` semantics. **Rejected**; a dedicated `mirror-check` keeps each
  guard single-purpose.

The primary (§3) is preferred because it (a) directly closes both evidence gaps —
the narrow test *and* the write-only-`.adw` generator — and (b) matches the
already-proven, reviewer-familiar `pack:check`/`pack:generate` pattern.

---

## 6. Risks & mitigations

| Risk | Mitigation |
|---|---|
| `verify` chain/order test (`scaffold.test.ts`) breaks when `mirror:check` is inserted | Update the required-stages list and ordering assertions in the same change (§3.4); run `scaffold.test.ts` first. |
| `mirror:sync --write` could clobber hand-authored `.claude/commands` content during review | `--check` is the default and the only thing `verify` runs; `--write` is opt-in. Canonical direction is documented (`.pi/prompts` → `.claude/commands`) and grounded in `DEFAULT_TEMPLATES_DIR`. `--dry-run` previews. |
| New `tools/` files accidentally shipped to `dist` | `tsconfig.build.json` includes only `src/`; `verify` ends with `rm -rf dist`. Same as `tools/parity-rate*.ts` today — no build change needed. |
| Byte-compare false-negatives from EOL/encoding | Use `Buffer.compare`, not string equality — exact bytes. |
| Symlink / non-regular entry sneaks into a tree and is silently skipped | `listFilesRecursive` throws on non-file/non-dir entries (fail loud), so a symlinked prompt can't masquerade as identical. |
| Helper reads the *repo* trees in unit tests, making them order/CWD-fragile | Pure-logic tests use `mkdtemp` temp dirs; only the explicit real-tree guard touches `.pi/prompts`/`.claude/commands`, via CWD-independent `resolveRepoPath`. |
| Scope creep into prompt content or `.adw/` | This change adds tooling/tests only; it must not edit prompt bodies or regenerate `.adw/prompts`. `pack:check` output is asserted unchanged. |

---

## 7. Verification

- **Focused first** (from `adw_sdlc/`):
  - `npx vitest run test/mirror.test.ts`
  - `npx vitest run test/phases.test.ts`
  - `npx vitest run test/scaffold.test.ts`
- **CLI smoke checks:**
  - `npm run mirror:check` ⇒ prints `mirror is byte-identical`, exit `0`.
  - `npm run mirror:sync` ⇒ reports `synced: 0` (already identical / no-op).
  - Negative spot check (manual, optional): temporarily edit one `.claude/commands`
    file, confirm `npm run mirror:check` exits `1` and lists it under `drifted`,
    then `npm run mirror:sync` restores it and `mirror:check` goes green again.
    Revert any scratch edit before finishing.
- **Full gate:** `npm run verify` from `adw_sdlc/` — must stay green. `typecheck`
  covers the new `tools/*.ts` (tsconfig includes `tools/`); `build` (src-only) is
  unaffected, so the new tool files never reach `dist`.

---

## 8. Open questions

- **O-1 (canonical direction).** Confirm `.pi/prompts` is the sync *source* and
  `.claude/commands` the *target*. This spec assumes yes, grounded in
  `DEFAULT_TEMPLATES_DIR = '.pi/prompts'` and the pack-generator comment calling
  `.pi/prompts` the single source of truth. If the team prefers a
  direction-agnostic check (no canonical), drop `mirror:sync` and ship `--check`
  only (still meets the issue AC, but leaves the mirror hand-maintained).
- **O-2 (compare scope).** Spec compares **all regular files** recursively. If the
  team wants to allow per-root local-only files (e.g. a `.claude/commands`-only
  README), introduce an explicit ignore list in `MIRROR_PAIRS`. Default: no
  ignores — both trees are 1:1 today, and an unflagged extra file is exactly the
  drift we want to catch.
- **O-3 (test placement).** Keep the neutrality regex in `phases.test.ts`
  (delegating byte-identity to the helper, as written), or consolidate both the
  byte-identity and neutrality checks into `mirror.test.ts` and remove the
  `phases.test.ts` block entirely? Recommendation: consolidate into
  `mirror.test.ts` for discoverability, leaving `phases.test.ts` focused on the
  phase catalog; either is acceptable.
- **O-4 (bash vs tsx).** If the maintainer prefers the minimal, no-new-TS-module
  route, Alternative A (a `diff -rq` bash gate like `lint:env`) satisfies the
  single issue AC with the least code. The tsx tool is recommended for the repair
  path and unit-testability; confirm preference.

---

## 9. Out of scope

- Any change to `src/` runtime behavior (orchestrator, run-phase, phases, state,
  runners, providers).
- Editing prompt **content** in any of the three trees, or regenerating
  `.adw/prompts` (no `pack:generate` run; `pack:check` stays green untouched).
- Adding new mirror pairs beyond `.pi/prompts` ↔ `.claude/commands` (the
  `MIRROR_PAIRS` list is structured to allow it later, but none are added here).
- Shipping the mirror tooling to `dist` or invoking it from the ADW runtime
  pipeline.
- Any git/gh automation — the orchestrator owns all git/gh; this tooling only
  reads/writes the two local prompt trees.
```
