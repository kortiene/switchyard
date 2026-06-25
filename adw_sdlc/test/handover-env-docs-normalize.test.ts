/**
 * Doc-normalization regression guard for issue #8.
 *
 * A docs-only chore normalized HANDOVER.md, HEALTHTECH_PORT.md, PARITY.md, and
 * docs/UNIVERSAL.md so that every `MX_AGENT_*` mention is unambiguously framed as
 * a deprecated compatibility alias — never presented as the canonical env knob.
 *
 * This guard encodes the acceptance criteria so that future doc edits cannot
 * silently regress them:
 *   #1 — no bare `MX_AGENT_[A-Z]+` (specific name) without framing context
 *   #2 — each doc's authority sentence stating canonical ADW_* / deprecated MX_AGENT_* is intact
 *   #5 — the security claim (MX_AGENT_* withheld/denied from runners) is preserved
 *   #7 — PLAN.md's historical banner is untouched
 *
 * Extends the src/-side guard in env-naming-drift.test.ts (issue #6) to docs.
 * The per-file `MX_AGENT_*` sweep from spec §4 acceptance criterion #1 is the
 * repeatable check an operator can run; this file makes it an automated invariant.
 *
 * PLAN.md is exempt from the no-bare-canonical check (historically uses MX_AGENT_*
 * names in the body) but must retain its "Reading note" / historical banner.
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';
import { ENV_ALIASES } from '../src/env-vars.js';

const DOCS_ROOT = join(REPO_ROOT, 'adw_sdlc');

// The four in-scope docs (PLAN.md is deliberately excluded from the bare-canonical check).
const IN_SCOPE_DOCS = {
  'HANDOVER.md': join(DOCS_ROOT, 'HANDOVER.md'),
  'HEALTHTECH_PORT.md': join(DOCS_ROOT, 'HEALTHTECH_PORT.md'),
  'PARITY.md': join(DOCS_ROOT, 'PARITY.md'),
  'docs/UNIVERSAL.md': join(DOCS_ROOT, 'docs', 'UNIVERSAL.md'),
} as const;

type DocKey = keyof typeof IN_SCOPE_DOCS;

/**
 * Framing tokens that render an `MX_AGENT_[A-Z]+` occurrence legitimate.
 * A line containing a specific legacy alias name (e.g. MX_AGENT_RUNNER) is a
 * "bare canonical" violation only if it has none of these tokens.
 */
const FRAMING_TOKENS = [
  'deprecated',
  'legacy',
  'alias',
  'deny',
  'denied',
  'withheld',
  'absent',
  'guard',
  'rename',
  'compatibility',
  '→',
  '->',
  'ENV_DENY_PREFIXES',
];

/**
 * True iff the line contains a specific MX_AGENT_[A-Z] env var name.
 *
 * Uses `[A-Z]` immediately after the underscore to exclude the wildcard forms
 * `MX_AGENT_*` and `MX_AGENT_'` that legitimately appear in the docs as prose
 * references to the deprecated-alias group without a specific name.
 */
function hasMxAgentSpecificRef(line: string): boolean {
  return /MX_AGENT_[A-Z][A-Z0-9_]*/.test(line);
}

/** True iff the line contains at least one framing token (case-insensitive). */
function hasFraming(line: string): boolean {
  const lower = line.toLowerCase();
  return FRAMING_TOKENS.some((tok) => lower.includes(tok.toLowerCase()));
}

interface Violation {
  doc: string;
  line: number;
  text: string;
}

/** Scan `content` for specific MX_AGENT_[A-Z]+ occurrences that lack framing context. */
function findBareCanonicalViolations(content: string, docName: string): Violation[] {
  const violations: Violation[] = [];
  content.split('\n').forEach((rawLine, index) => {
    if (hasMxAgentSpecificRef(rawLine) && !hasFraming(rawLine)) {
      violations.push({ doc: docName, line: index + 1, text: rawLine.trim() });
    }
  });
  return violations;
}

// ---------------------------------------------------------------------------
// Acceptance criterion #1 — no bare MX_AGENT_[A-Z]+ as canonical
// ---------------------------------------------------------------------------

describe('doc-normalization guard — no bare MX_AGENT_* as canonical in the four in-scope docs', () => {
  it('every specific MX_AGENT_[A-Z]+ name in the four docs carries framing context', () => {
    const violations: Violation[] = [];
    for (const [docName, docPath] of Object.entries(IN_SCOPE_DOCS) as [DocKey, string][]) {
      const content = readFileSync(docPath, 'utf8');
      violations.push(...findBareCanonicalViolations(content, docName));
    }
    const report = violations.map((v) => `  ${v.doc}:${v.line}: ${v.text}`).join('\n');
    expect(
      violations,
      `bare MX_AGENT_<NAME> occurrence(s) found without framing (deprecated/legacy/alias/deny) — ` +
        `every legacy alias name must be visibly framed, never presented as the canonical knob:\n${report}`,
    ).toEqual([]);
  });

  it('all four in-scope docs are readable and non-empty', () => {
    for (const [docName, docPath] of Object.entries(IN_SCOPE_DOCS) as [DocKey, string][]) {
      const content = readFileSync(docPath, 'utf8');
      expect(content.length, `${docName} must not be empty`).toBeGreaterThan(200);
    }
  });
});

// ---------------------------------------------------------------------------
// Detector self-tests
// ---------------------------------------------------------------------------

describe('doc-normalization guard — detector self-test', () => {
  it('fires on a bare MX_AGENT_RUNNER instruction (no framing)', () => {
    expect(hasMxAgentSpecificRef('Set MX_AGENT_RUNNER=claude to choose the runner.')).toBe(true);
    expect(hasFraming('Set MX_AGENT_RUNNER=claude to choose the runner.')).toBe(false);
    expect(findBareCanonicalViolations('Set MX_AGENT_RUNNER=claude to choose the runner.', 'fake.md')).toHaveLength(
      1,
    );
  });

  it('does not fire on legitimate wildcard forms (`MX_AGENT_*`, `MX_AGENT_`)', () => {
    // The wildcard form `MX_AGENT_*` does not have [A-Z] immediately after the underscore
    expect(hasMxAgentSpecificRef('Use `ADW_*` (legacy `MX_AGENT_*`) knobs.')).toBe(false);
    expect(hasMxAgentSpecificRef("ENV_DENY_PREFIXES = ['MATRIX_', 'MX_AGENT_', 'ADW_']")).toBe(false);
    expect(hasMxAgentSpecificRef('deny-prefixed (`MATRIX_`/`ADW_`/legacy `MX_AGENT_`)')).toBe(false);
  });

  it('does not fire on framed specific-name lines (deprecated / legacy / alias / deny)', () => {
    for (const framedLine of [
      'The deprecated `MX_AGENT_RUNNER` alias is still accepted.',
      'Use `ADW_RUNNER` (legacy alias `MX_AGENT_RUNNER`) instead.',
      '`ADW_*` and legacy `MX_AGENT_ENGINE` keys are withheld from runners.',
      'Removing the denied `MX_AGENT_TEST_CMD` prefix needs a review.',
      'The rename was MX_AGENT_FINALIZE_GATES → ADW_FINALIZE_GATES.',
      "compatibility `MX_AGENT_RUNNER` aliases remain denied from runner children.",
    ]) {
      expect(
        findBareCanonicalViolations(framedLine, 'fake.md'),
        `expected no violation for: ${framedLine}`,
      ).toEqual([]);
    }
  });

  it('fires on instructional text presenting MX_AGENT_* as the name to use', () => {
    for (const instructionalLine of [
      'Export MX_AGENT_ENGINE=codex before running.',
      'Run with MX_AGENT_TEST_CMD="npm test".',
      'Set MX_AGENT_CLASSIFY_ON_RUNNER=1 to classify on the runner.',
    ]) {
      expect(
        findBareCanonicalViolations(instructionalLine, 'fake.md'),
        `expected violation for: ${instructionalLine}`,
      ).toHaveLength(1);
    }
  });
});

// ---------------------------------------------------------------------------
// Acceptance criterion #2 — per-doc authority statement present
// ---------------------------------------------------------------------------

describe('doc-normalization guard — per-doc authority statement', () => {
  it('HANDOVER.md states that canonical env knobs use ADW_* and MX_AGENT_* aliases are deprecated', () => {
    const content = readFileSync(IN_SCOPE_DOCS['HANDOVER.md'], 'utf8');
    // "Canonical env knobs use `ADW_*`; deprecated compatibility `MX_AGENT_*` aliases…"
    expect(content).toMatch(/Canonical env knobs use `ADW_\*`/);
    expect(content).toMatch(/deprecated\s+compatibility\s+`MX_AGENT_\*`\s+aliases/);
  });

  it('HEALTHTECH_PORT.md states ADW_* is canonical and MX_AGENT_* are deprecated compatibility aliases', () => {
    const content = readFileSync(IN_SCOPE_DOCS['HEALTHTECH_PORT.md'], 'utf8');
    expect(content).toMatch(/canonical\s+`ADW_\*`/i);
    expect(content).toMatch(/deprecated compatibility aliases/i);
  });

  it('PARITY.md annotates MX_AGENT_* as legacy in the env-deny context', () => {
    const content = readFileSync(IN_SCOPE_DOCS['PARITY.md'], 'utf8');
    expect(content).toMatch(/legacy\s+`MX_AGENT_\*`/i);
  });

  it('docs/UNIVERSAL.md annotates MX_AGENT_* as legacy and states keys are withheld from runners', () => {
    const content = readFileSync(IN_SCOPE_DOCS['docs/UNIVERSAL.md'], 'utf8');
    expect(content).toMatch(/legacy\s+`MX_AGENT_/i);
    expect(content).toMatch(/MX_AGENT_\*`?\s+keys\s+are\s+withheld/i);
  });
});

// ---------------------------------------------------------------------------
// Acceptance criterion #5 — security claim intact
// ---------------------------------------------------------------------------

describe('doc-normalization guard — security claim intact', () => {
  it('HANDOVER.md states MX_AGENT_* aliases remain denied from runner children', () => {
    const content = readFileSync(IN_SCOPE_DOCS['HANDOVER.md'], 'utf8');
    // The invariant text at §5 and §8 both mention "denied from runner children"
    expect(content).toMatch(/MX_AGENT_\*`\s+aliases\s+remain\s+denied\s+from\s+runner\s+children/);
  });

  it('HANDOVER.md also records MX_AGENT_* aliases in the rebranding invariant', () => {
    const content = readFileSync(IN_SCOPE_DOCS['HANDOVER.md'], 'utf8');
    // Rebranding invariant: "deprecated compatibility `MX_AGENT_*` aliases. Removing either denied…"
    expect(content).toMatch(/deprecated compatibility `MX_AGENT_\*` aliases\. Removing either denied/);
  });

  it('docs/UNIVERSAL.md states MX_AGENT_* keys are withheld from runner children', () => {
    const content = readFileSync(IN_SCOPE_DOCS['docs/UNIVERSAL.md'], 'utf8');
    expect(content).toMatch(/`MX_AGENT_\*`?\s+keys\s+are\s+withheld\s+from\s+runner\s+children/i);
  });
});

// ---------------------------------------------------------------------------
// Acceptance criterion #7 — PLAN.md historical banner intact
// ---------------------------------------------------------------------------

describe('doc-normalization guard — PLAN.md historical banner', () => {
  it('PLAN.md top-of-file banner carries the "Reading note" historical marker', () => {
    const content = readFileSync(join(DOCS_ROOT, 'PLAN.md'), 'utf8');
    const top = content.slice(0, 2000);
    expect(top).toMatch(/Reading note/i);
  });

  it('PLAN.md banner acknowledges the MX_AGENT_* → ADW_* rename and marks MX_AGENT_* as deprecated', () => {
    const content = readFileSync(join(DOCS_ROOT, 'PLAN.md'), 'utf8');
    const top = content.slice(0, 2000);
    // "Runtime control-plane env vars are now canonicalized under `ADW_*`; the
    //  historical `MX_AGENT_*` names below are deprecated compatibility aliases…"
    expect(top).toMatch(/canonicalized under `ADW_\*`/);
    expect(top).toMatch(/`MX_AGENT_\*`.*deprecated compatibility aliases/s);
  });

  it('PLAN.md banner states MX_AGENT_* names are denied from runner subprocesses', () => {
    const content = readFileSync(join(DOCS_ROOT, 'PLAN.md'), 'utf8');
    const top = content.slice(0, 2000);
    expect(top).toMatch(/denied from runner subprocesses/i);
  });
});

// ---------------------------------------------------------------------------
// Irregular pair canonical-name accuracy (spec §1 table — non-mechanical pairs)
// ---------------------------------------------------------------------------

describe('doc-normalization guard — irregular pair canonical names', () => {
  it('the four docs do not use MX_AGENT_ASSUME_YES (correct legacy alias is MX_AGENT_YES)', () => {
    for (const [docName, docPath] of Object.entries(IN_SCOPE_DOCS) as [DocKey, string][]) {
      const content = readFileSync(docPath, 'utf8');
      expect(content, `${docName} must not contain the wrong alias MX_AGENT_ASSUME_YES`).not.toContain(
        'MX_AGENT_ASSUME_YES',
      );
    }
  });

  it('the four docs do not use ADW_FORCE_FENCED without PARITY (correct canonical is ADW_PARITY_FORCE_FENCED_JSON)', () => {
    // ADW_PARITY_FORCE_FENCED_JSON is a substring that contains "ADW_" + "FORCE_FENCED" but also "PARITY_"
    // The wrong forms would be: ADW_FORCE_FENCED (no PARITY prefix) or ADW_PARITY_FORCE_FENCED (no _JSON)
    for (const [docName, docPath] of Object.entries(IN_SCOPE_DOCS) as [DocKey, string][]) {
      const content = readFileSync(docPath, 'utf8');
      // Neither of the two truncated wrong forms should appear
      expect(content, `${docName} must not contain abbreviated ADW_FORCE_FENCED`).not.toMatch(
        /\bADW_FORCE_FENCED\b/,
      );
      expect(
        content,
        `${docName} must not contain ADW_PARITY_FORCE_FENCED without _JSON suffix`,
      ).not.toMatch(/\bADW_PARITY_FORCE_FENCED\b(?!_JSON)/);
    }
  });

  it('the four docs do not use MX_AGENT_PARITY_FORCE_FENCED (correct legacy alias is MX_AGENT_FORCE_FENCED)', () => {
    for (const [docName, docPath] of Object.entries(IN_SCOPE_DOCS) as [DocKey, string][]) {
      const content = readFileSync(docPath, 'utf8');
      expect(
        content,
        `${docName} must not contain the wrong alias MX_AGENT_PARITY_FORCE_FENCED`,
      ).not.toContain('MX_AGENT_PARITY_FORCE_FENCED');
    }
  });
});

// ---------------------------------------------------------------------------
// Tied to ENV_ALIASES source of truth
// ---------------------------------------------------------------------------

describe('doc-normalization guard — tied to ENV_ALIASES source of truth', () => {
  it('the detector fires on a bare read of each legacy alias name from ENV_ALIASES', () => {
    const legacyNames = Object.values(ENV_ALIASES).map((alias) => alias.legacy);
    expect(legacyNames.length).toBeGreaterThan(0);

    for (const legacy of legacyNames) {
      // A bare instructional line with each legacy name should be caught
      const instructional = `Set ${legacy}=value before running.`;
      expect(
        findBareCanonicalViolations(instructional, 'fake.md'),
        `detector must fire on bare instructional use of ${legacy}`,
      ).toHaveLength(1);
    }
  });

  it('the detector does not fire on framed legacy alias names from ENV_ALIASES', () => {
    const legacyNames = Object.values(ENV_ALIASES).map((alias) => alias.legacy);
    expect(legacyNames.length).toBeGreaterThan(0);

    for (const legacy of legacyNames) {
      const framed = `The deprecated ${legacy} alias is still accepted as input.`;
      expect(
        findBareCanonicalViolations(framed, 'fake.md'),
        `detector must NOT fire on framed use of ${legacy}`,
      ).toEqual([]);
    }
  });

  it('ENV_ALIASES canonical names are all ADW_-prefixed (source of truth sanity)', () => {
    for (const [key, alias] of Object.entries(ENV_ALIASES)) {
      expect(alias.canonical, `${key} canonical name must start with ADW_`).toMatch(/^ADW_/);
      expect(alias.legacy, `${key} legacy name must start with MX_AGENT_`).toMatch(/^MX_AGENT_/);
    }
  });
});
