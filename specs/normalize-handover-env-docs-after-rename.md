# Spec: Normalize handover/env docs after the `ADW_*` rename

- **Issue:** #8 — `chore: normalize handover/env docs after rename`
- **Labels:** `issue_class:chore`, `adw-live-batch`
- **Planned ADW run mode:** native
- **Type:** docs-only cleanup (no production behavior change)
- **In-scope files:** `adw_sdlc/HANDOVER.md`, `adw_sdlc/HEALTHTECH_PORT.md`, `adw_sdlc/PARITY.md`, `adw_sdlc/docs/UNIVERSAL.md`
- **Explicitly out of scope:** `adw_sdlc/PLAN.md` (historical body, already annotated)
- **Source of truth for names:** `adw_sdlc/src/env-vars.ts` (`ENV_ALIASES`, `modelEnvAlias`)
- **Pairs with:** Issue #6 drift guard — `specs/drift-guard-adw-env-naming.md` (covers `src/`; this chore covers docs)
- **Local gate:** `npm run verify` (from `adw_sdlc/`)

---

## 1. Context & current state (read this first)

A recent migration renamed the control-plane env knobs from `MX_AGENT_*` to
canonical `ADW_*`, keeping `MX_AGENT_*` as **deprecated compatibility aliases**
that are still accepted as input but remain denied from runner subprocesses. The
alias machinery lives entirely in `adw_sdlc/src/env-vars.ts`:

- `ENV_ALIASES` — the 7 canonical/legacy pairs.
- `modelEnvAlias(phase)` — the per-phase `ADW_MODEL_<PHASE>` / `MX_AGENT_MODEL_<PHASE>` pair.
- `readEnvAlias` / `readEnvFlag` — the only sanctioned readers: prefer canonical,
  accept the legacy alias with a one-time stderr deprecation warning, and **throw**
  if canonical and legacy disagree.

### Canonical ⇄ deprecated mapping (authoritative; copy from `env-vars.ts`)

| Knob | Canonical (`ADW_*`) | Deprecated alias (`MX_AGENT_*`) |
|---|---|---|
| engine | `ADW_ENGINE` | `MX_AGENT_ENGINE` |
| runner | `ADW_RUNNER` | `MX_AGENT_RUNNER` |
| test command | `ADW_TEST_CMD` | `MX_AGENT_TEST_CMD` |
| finalize gates | `ADW_FINALIZE_GATES` | `MX_AGENT_FINALIZE_GATES` |
| classify-on-runner | `ADW_CLASSIFY_ON_RUNNER` | `MX_AGENT_CLASSIFY_ON_RUNNER` |
| assume-yes | `ADW_ASSUME_YES` | `MX_AGENT_YES` |
| force fenced JSON | `ADW_PARITY_FORCE_FENCED_JSON` | `MX_AGENT_FORCE_FENCED` |
| per-phase model | `ADW_MODEL_<PHASE>` | `MX_AGENT_MODEL_<PHASE>` |

> Note the two non-mechanical pairs: assume-yes is `ADW_ASSUME_YES` ⇄ `MX_AGENT_YES`
> (not `MX_AGENT_ASSUME_YES`), and force-fenced is `ADW_PARITY_FORCE_FENCED_JSON`
> ⇄ `MX_AGENT_FORCE_FENCED` (the prefixes are not a 1:1 token swap). The
> deny-prefix constant in `src/env.ts` is `ENV_DENY_PREFIXES = ['MATRIX_', 'MX_AGENT_', 'ADW_']`.

The canonical/deprecated relationship is already stated cleanly in the **out-of-scope
but exemplary** `adw_sdlc/README.md` (it calls `ADW_*` canonical, names `MX_AGENT_*`
deprecated aliases, and notes both are withheld from runners). Use that wording as
the reference voice; do **not** edit README under this issue.

### Audited current state of the four in-scope docs

A full sweep (`MX_AGENT_[A-Z_]*` and `MX_AGENT_`) shows **every** `MX_AGENT_`
occurrence in the four in-scope docs already appears in a *deprecated-alias* or
*deny-prefix* context — **none presents a bare `MX_AGENT_*` as the canonical name**.
Inventory at the time of writing (verify line numbers before editing; the files
change often):

| File | Line(s) | Form today | Relationship stated? | Action |
|---|---|---|---|---|
| `HANDOVER.md` | ~152 | `ENV_DENY_PREFIXES = ['MATRIX_', 'MX_AGENT_', 'ADW_']` (code constant) | Implied by surrounding §; not at the line | Keep verbatim (must mirror `src/env.ts`); ensure nearby prose states canonical/deprecated |
| `HANDOVER.md` | ~166 | "Canonical env knobs use `ADW_*`; inherited `MX_AGENT_*` aliases remain denied" | ✅ explicit | Keep |
| `HANDOVER.md` | ~1179 | "guard for the `MX_AGENT_*` → `ADW_*` env rename" | ✅ explicit (rename direction) | Keep |
| `HANDOVER.md` | ~1184 | "bare … reads of `MX_AGENT_*` keys" (describing the guard) | n/a (guard description) | Keep |
| `HANDOVER.md` | ~1197 | deny list `GH_TOKEN / MATRIX_* / ADW_* / MX_AGENT_*` | n/a (deny context) | Keep |
| `HANDOVER.md` | ~1319 | "canonical `ADW_*` names now exist with deprecated `MX_AGENT_*` aliases" | ✅ explicit | Keep |
| `HEALTHTECH_PORT.md` | ~66 | "canonical `ADW_*` namespace; inherited `MX_AGENT_*` … deprecated compatibility aliases" | ✅ explicit | Keep |
| `PARITY.md` | ~30 | deny list "…`ADW_*`/legacy `MX_AGENT_*`" | ✅ annotated "legacy" | Keep |
| `docs/UNIVERSAL.md` | ~176 | "deny-prefixed (`MATRIX_`/`ADW_`/legacy `MX_AGENT_`)" | ✅ annotated "legacy" | Keep |
| `docs/UNIVERSAL.md` | ~517 | "`ADW_*`, and legacy `MX_AGENT_*` keys are withheld" | ✅ annotated "legacy" | Keep |

**Consequence for this issue:** acceptance criterion #1 ("no bare `MX_AGENT_*` as
canonical") is *already met* by inspection. The real, non-trivial deliverable of
this chore is therefore:

1. an **audited, repeatable verification** that the criterion holds (so it does not
   silently regress), and
2. **targeted phrasing/consistency touch-ups** so every env reference is
   self-evidently unambiguous (one house phrasing for "deprecated alias"), and
3. confirming **cross-references still resolve** after any touch-ups.

Be honest in the PR/run summary that the docs were largely compliant already; the
diff should be small and surgical, not a rewrite.

---

## 2. Normalization policy (the rule to apply)

Adopt this pragmatic interpretation of "each clearly states canonical `ADW_*`
with `MX_AGENT_*` as a deprecated alias" — encode it in the spec so the
implementer does not over-edit:

1. **No bare canonical `MX_AGENT_*`.** No env reference in the four docs may
   present an `MX_AGENT_*` name as *the* name to use. Every `MX_AGENT_*` mention
   must be visibly framed as legacy/deprecated/alias, OR be part of a deny-prefix
   enumeration (where it is correct to list the denied prefix literally).
2. **Per-doc authority statement.** Each in-scope doc that mentions control-plane
   env knobs at all must contain at least one clear sentence establishing the
   canonical/deprecated relationship (the README sentence is the model). The four
   docs already satisfy this; preserve it.
3. **Do NOT require every `ADW_*` mention to repeat the alias.** Repeating
   "(deprecated alias: `MX_AGENT_*`)" on every canonical mention is noise and is
   *not* wanted. Mention the alias where the relationship is first/best explained
   per doc; elsewhere use the bare canonical `ADW_*` name.
4. **House phrasing.** When touching a deprecation note, prefer one consistent
   form. Recommended canonical phrasing:
   > canonical `ADW_*` (deprecated compatibility alias: `MX_AGENT_*`, still
   > accepted as input but withheld from runner subprocesses)
   Shorter inline form where space is tight: `` `ADW_*` (legacy alias `MX_AGENT_*`) ``.
   Do not invent a third synonym; "legacy" and "deprecated compatibility alias"
   already coexist — converge toward "deprecated compatibility alias" for the
   authority sentence and "legacy alias" for inline deny-list annotations.
5. **Names must match `env-vars.ts`.** Any specific knob named in docs must use the
   exact canonical token from the §1 table (watch `ADW_ASSUME_YES` vs `MX_AGENT_YES`
   and `ADW_PARITY_FORCE_FENCED_JSON` vs `MX_AGENT_FORCE_FENCED`).
6. **Never weaken the security statement.** `MX_AGENT_*` (and `ADW_*`) are denied
   from runner children; any edit must preserve that claim. Deny-prefix
   enumerations that list `'MX_AGENT_'` literally are correct and must stay.
7. **Leave code-literal lines verbatim.** Lines that reproduce a source constant
   (e.g. `ENV_DENY_PREFIXES = [...]`) must keep matching `src/env.ts`; clarify in
   surrounding prose, not by mutating the literal.

---

## 3. Implementation steps

> Docs-only. Do not modify any file under `adw_sdlc/src/`, `adw_sdlc/test/`,
> `adw_sdlc/tools/`, or `.adw/`. The orchestrator owns all git/gh; this phase only
> edits Markdown.

### Step 1 — Re-run the audit to refresh line numbers

From `adw_sdlc/`, enumerate every env-name occurrence in the four in-scope files
so edits target the live text (the inventory in §1 is a snapshot):

```bash
# every legacy occurrence, with the suffix, in scope files only
rg -n 'MX_AGENT_[A-Z_]*|MX_AGENT_' HANDOVER.md HEALTHTECH_PORT.md PARITY.md docs/UNIVERSAL.md
# canonical occurrences, to spot any specific knob that is misspelled or stale
rg -n 'ADW_[A-Z_]+' HANDOVER.md HEALTHTECH_PORT.md PARITY.md docs/UNIVERSAL.md
```

For each `MX_AGENT_` hit, classify it as one of: **(a)** authority sentence,
**(b)** inline deny-prefix annotation, **(c)** rename/guard description, or
**(d)** raw code-literal. Only (a)/(b) are candidates for phrasing touch-ups; (c)
and (d) are kept verbatim unless factually wrong.

### Step 2 — Apply the normalization policy (surgical edits only)

For each occurrence, apply §2:

- If any hit presents `MX_AGENT_*` as canonical (none expected today) → rewrite so
  the canonical `ADW_*` name leads and `MX_AGENT_*` is the parenthetical alias.
- If a doc mentions env knobs but lacks a clear authority sentence → add one
  (use the house phrasing from §2.4). Expected: no additions needed; verify.
- If deprecation phrasing is inconsistent within a doc (e.g. mixes "inherited",
  "legacy", "deprecated") in a way that reads ambiguously → converge to the house
  phrasing. Keep the change minimal; do not reflow unrelated prose.
- Verify each named knob spells the canonical token exactly per the §1 table.

Do **not** touch `PLAN.md`. Confirm its top-of-file historical banner
(lines ~3–20) already states the `MX_AGENT_*` → `ADW_*` deprecation; leave the
historical body intact.

### Step 3 — Verify cross-references resolve

The "cross-references resolve" criterion covers links *within and between* the
edited docs. After edits:

```bash
# intra-doc anchors and relative links that appear in the edited files
rg -n '\]\((#|\./|\.\./)' HANDOVER.md HEALTHTECH_PORT.md PARITY.md docs/UNIVERSAL.md
```

For each result confirm:
- **Anchor links** (`](#some-heading)`) still match a heading in the same file
  (heading text was not renamed by an edit; e.g. PARITY's
  `#structured-output-hard-failure-rate`).
- **Relative links** (`](./x.md)`, `](../x.md)`) point at files that exist.
- Any doc that points readers to "see `env-vars.ts`" / `README.md` / sibling docs
  for env naming still names an existing target.

If an edit renamed a heading that another doc anchors to, restore the heading or
update the referrer in the same change.

### Step 4 — Run the local gate

```bash
cd adw_sdlc
npm run verify   # typecheck → lint:env → pack:check → test → build → rm -rf dist
```

`verify` must stay green. It does not lint Markdown prose, so green here proves the
docs change broke nothing in code/tests/prompt-pack; the doc-quality checks are
the manual audits in Steps 1–3.

---

## 4. Acceptance criteria

Mirrors the issue, made concrete:

1. **No bare canonical `MX_AGENT_*`.** `rg -n 'MX_AGENT_[A-Z_]+' HANDOVER.md
   HEALTHTECH_PORT.md PARITY.md docs/UNIVERSAL.md` returns only occurrences that
   are visibly framed as deprecated/legacy aliases, deny-prefix enumerations, or
   rename/guard descriptions — never an instruction to use an `MX_AGENT_*` name.
   `PLAN.md` is exempt (historical).
2. **Per-doc authority statement present.** Each of the four docs that references
   control-plane env knobs contains ≥1 clear canonical-vs-deprecated statement.
3. **Cross-references resolve.** All anchor and relative links in the four edited
   docs point at existing headings/files (Step 3 check passes).
4. **Names accurate.** Every specific knob named matches the canonical token in the
   §1 table / `src/env-vars.ts`.
5. **Security claim intact.** Every place that previously said `MX_AGENT_*`/`ADW_*`
   are withheld from runners still says so.
6. **`npm run verify` is green** from `adw_sdlc/`.
7. **`PLAN.md` unchanged.**

---

## 5. Risks & mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Over-editing turns a small chore into a noisy diff | Medium | §2.3/§2.4 cap edits to authority sentences + inline annotations; no prose reflow. |
| Editing a code-literal line (`ENV_DENY_PREFIXES`) so it diverges from `src/env.ts` | Low | §2.7: clarify in prose, never mutate the literal; the #6 drift test guards `src/`, not docs. |
| Renaming a heading breaks an anchor another doc relies on | Low | Step 3 cross-reference check; prefer not to rename headings. |
| Wrong alias token (e.g. `MX_AGENT_ASSUME_YES`, `MX_AGENT_PARITY_FORCE_FENCED_JSON`) introduced | Low | §1 table is authoritative; the two irregular pairs called out explicitly. |
| Accidentally touching `PLAN.md` or files outside the four | Low | Scope list + acceptance #7; review `git status` is the orchestrator's job, but keep edits to the four paths. |
| Weakening the secret-boundary wording while "simplifying" | Low | §2.6 + acceptance #5 forbid it. |

---

## 6. Test strategy

- **No new automated tests.** This is docs-only; the behavioral guard for env
  naming in `src/` is the separate #6 drift test (`test/env-naming-drift.test.ts`),
  which does not scan docs and is unaffected.
- **Regression posture:** the `MX_AGENT_[A-Z_]+` sweep in acceptance #1 is the
  repeatable check an operator (or a future guard) can run against the four docs.
  *Optional follow-up, not part of this issue:* extend the #6-style guard to assert
  these docs never present a bare canonical `MX_AGENT_*` — note it in the run
  summary as a suggestion; do not implement it here.
- **Gate:** `npm run verify` proves the prompt-pack/build/tests are untouched.

---

## 7. Rollout / rollback

- **Rollout:** single docs-only change; merges with the normal phased pipeline.
  No migration, no flags, no runtime impact.
- **Rollback:** revert the commit; pure documentation, zero blast radius.

---

## Summary of key decisions

- Treat the issue as **audit + surgical consistency touch-ups**, not a rewrite,
  because the four docs already frame `MX_AGENT_*` as deprecated everywhere
  (acceptance #1 already holds by inspection).
- Adopt an explicit **normalization policy** (§2) that forbids requiring every
  `ADW_*` mention to repeat the alias — preventing diff noise while keeping each
  doc unambiguous.
- Anchor all names to `src/env-vars.ts` (`ENV_ALIASES` + `modelEnvAlias`) and call
  out the two irregular pairs (`ADW_ASSUME_YES`⇄`MX_AGENT_YES`,
  `ADW_PARITY_FORCE_FENCED_JSON`⇄`MX_AGENT_FORCE_FENCED`).
- Keep code-literal lines (`ENV_DENY_PREFIXES`) verbatim; clarify in prose only.
- `PLAN.md` stays untouched; its historical banner already records the rename.

## Assumptions

- "Cross-references resolve" means intra-/inter-doc Markdown links and anchors in
  the edited files, not external URLs.
- README is intentionally out of scope (not listed in the issue) and serves only
  as the wording model.
- `npm run verify` does not lint Markdown, so doc-quality is enforced by the manual
  audits in §3; green `verify` only proves nothing else broke.
- The implementer may run `git`/`rg` read-only locally; per ADW rules the
  orchestrator performs all commits/branches/PRs.

## Open questions

- Should a future automated guard assert "no bare canonical `MX_AGENT_*`" in docs
  (mirroring #6 for `src/`)? Flagged as optional follow-up in §6; left out of this
  chore to keep it docs-only.
- Preferred single house phrasing: this spec recommends "deprecated compatibility
  alias" for authority sentences and "legacy alias" for inline deny-list notes —
  confirm if the maintainer wants strict single-term convergence instead.
