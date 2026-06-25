/**
 * Env-rename drift guard (issue #6).
 *
 * A recent migration renamed the control-plane env knobs from `MX_AGENT_*` to
 * canonical `ADW_*`, keeping `MX_AGENT_*` working as *deprecated compatibility
 * aliases*. The alias machinery — `ENV_ALIASES`, `modelEnvAlias`, and the only
 * sanctioned readers `readEnvAlias`/`readEnvFlag` — lives entirely in
 * `src/env-vars.ts`.
 *
 * This guard is a pure regression preventer: every `src/**\/*.ts` file except
 * `env-vars.ts` (the sanctioned home of the alias table) is scanned for a bare
 * read of an `MX_AGENT_*` key, i.e. an env-object access via bracket index
 * (`env['MX_AGENT_X']`) or dot member (`process.env.MX_AGENT_X`). It must fail
 * only when a *new* such read is introduced — today's tree is green because all
 * control-plane reads route through the `env-vars.ts` helpers.
 *
 * Discriminator: a real read always has an uppercase letter immediately after
 * the `MX_AGENT_` prefix AND uses bracket/dot access. That excludes every
 * legitimate mention in the current tree by construction — the deny-prefix
 * constant `'MX_AGENT_'` (suffix is the closing quote), the `--help` text
 * `(deprecated alias: MX_AGENT_RUNNER)` (no `.`/`[` access), the prose comments
 * `MX_AGENT_-prefixed` / `MX_AGENT_*` (suffix is `-`/`*`), and the
 * `legacy: 'MX_AGENT_ENGINE'` object values (in the excluded file).
 *
 * Comment handling: each line is truncated at its first `//` before matching, so
 * a commented-out example read does not trip the guard. This is intentionally
 * pragmatic (it can also truncate a `//` inside a string, e.g. a URL) — no
 * legitimate `src/` line places a real `MX_AGENT_[A-Z]` read after a `//`, so
 * the simplicity is safe. Block comments are not stripped; none in `src/` matter.
 */

import { readdirSync, readFileSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

import { describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';
import { ENV_ALIASES, modelEnvAlias } from '../src/env-vars.js';

const SRC_DIR = join(REPO_ROOT, 'adw_sdlc', 'src');
const SANCTIONED_FILE = 'env-vars.ts';

// Bracket index:  env['MX_AGENT_RUNNER']  /  process.env["MX_AGENT_FOO"]
const BRACKET = /\[\s*['"`]MX_AGENT_[A-Z][A-Z0-9_]*['"`]\s*\]/;
// Dot member:     process.env.MX_AGENT_RUNNER
const DOT = /\.\s*MX_AGENT_[A-Z][A-Z0-9_]*/;

/** True iff the (comment-stripped) line bare-reads an `MX_AGENT_*` env key. */
function isBareRead(line: string): boolean {
  return BRACKET.test(line) || DOT.test(line);
}

/** Drop everything from the first `//` to end of line. */
function stripLineComment(line: string): string {
  const idx = line.indexOf('//');
  return idx === -1 ? line : line.slice(0, idx);
}

interface Violation {
  file: string;
  line: number;
  text: string;
}

/** Collect bare `MX_AGENT_*` reads in `text`, attributing each to `file`. */
function findViolations(text: string, file: string): Violation[] {
  const violations: Violation[] = [];
  text.split('\n').forEach((rawLine, index) => {
    if (isBareRead(stripLineComment(rawLine))) {
      violations.push({ file, line: index + 1, text: rawLine.trim() });
    }
  });
  return violations;
}

/** Enumerate every `src/**\/*.ts` file except the sanctioned `env-vars.ts`. */
function scannedSourceFiles(): string[] {
  return readdirSync(SRC_DIR, { recursive: true, encoding: 'utf8' })
    .filter((p) => p.endsWith('.ts'))
    .map((p) => p.split('/').join(sep))
    .filter((p) => p !== SANCTIONED_FILE && !p.endsWith(`${sep}${SANCTIONED_FILE}`));
}

describe('env-naming drift guard — no bare MX_AGENT_* reads in src/', () => {
  it('every src/ file (except env-vars.ts) reads control-plane env via the alias helpers', () => {
    const files = scannedSourceFiles();
    // Sanity: the scan actually found the source tree (not an empty no-op).
    expect(files.length).toBeGreaterThan(0);

    const violations: Violation[] = [];
    for (const rel of files) {
      const abs = join(SRC_DIR, rel);
      const text = readFileSync(abs, 'utf8');
      for (const v of findViolations(text, relative(REPO_ROOT, abs))) {
        violations.push(v);
      }
    }

    const report = violations.map((v) => `  ${v.file}:${v.line}: ${v.text}`).join('\n');
    expect(
      violations,
      `bare MX_AGENT_* env read(s) found outside env-vars.ts — read control-plane env ` +
        `via the env-vars.ts alias helpers (readEnvAlias/readEnvFlag) instead:\n${report}`,
    ).toEqual([]);
  });
});

describe('env-naming drift guard — detector self-test', () => {
  it('fires on bare MX_AGENT_* reads (positive fixtures)', () => {
    for (const positive of [
      "process.env['MX_AGENT_RUNNER']",
      'env["MX_AGENT_FOO"]',
      'process.env.MX_AGENT_RUNNER',
      "source['MX_AGENT_MODEL_IMPLEMENT']",
      'deps.env.MX_AGENT_MODEL_IMPLEMENT',
    ]) {
      expect(isBareRead(positive), `expected detector to fire on: ${positive}`).toBe(true);
    }
  });

  it('does not fire on legitimate mentions (negative / known-good fixtures)', () => {
    for (const negative of [
      "ENV_DENY_PREFIXES = ['MATRIX_', 'MX_AGENT_', 'ADW_'] as const;", // deny-prefix constant
      'Env: ADW_RUNNER (deprecated alias: MX_AGENT_RUNNER)', // cli --help text
      ' * MATRIX_-/ADW_-/MX_AGENT_-prefixed key in phased mode.', // prose comment
      'including GH_TOKEN and MATRIX_*-/ADW_-/MX_AGENT_*-prefixed', // prose comment
      'readEnvAlias(env, ENV_ALIASES.runner)', // canonical usage
      "process.env['ADW_RUNNER']", // canonical usage
      "engine: { canonical: 'ADW_ENGINE', legacy: 'MX_AGENT_ENGINE' },", // alias-table value
    ]) {
      expect(isBareRead(negative), `expected detector NOT to fire on: ${negative}`).toBe(false);
    }
  });

  it('still flags a real read carrying a trailing // comment', () => {
    expect(findViolations("const x = process.env['MX_AGENT_RUNNER']; // legacy", 'fixture.ts')).toHaveLength(1);
  });

  it('ignores a fully commented-out example read (// stripped before matching)', () => {
    expect(findViolations("// const x = process.env['MX_AGENT_RUNNER'];", 'fixture.ts')).toEqual([]);
  });
});

describe('env-naming drift guard — tied to the ENV_ALIASES source of truth', () => {
  it('catches a bare read of every legacy alias name (incl. per-phase model alias)', () => {
    const legacyNames = [
      ...Object.values(ENV_ALIASES).map((alias) => alias.legacy),
      modelEnvAlias('implement').legacy,
    ];
    // Guard against an empty list silently passing the assertion below.
    expect(legacyNames.length).toBeGreaterThan(0);

    for (const legacy of legacyNames) {
      expect(isBareRead(`const v = env['${legacy}'];`), `bracket read of ${legacy} should be caught`).toBe(true);
      expect(isBareRead(`const v = process.env.${legacy};`), `dot read of ${legacy} should be caught`).toBe(true);
    }
  });
});
