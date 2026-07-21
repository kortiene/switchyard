import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { beforeAll, describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';
import { ENGINE, RUNNER_IDS } from '../src/index.js';

describe('scaffold', () => {
  it('exposes the ts engine identity and the four runner ids', () => {
    expect(ENGINE).toBe('ts');
    expect(RUNNER_IDS).toEqual(['claude', 'codex', 'opencode', 'pi']);
  });

  it('keeps the D3 package invariants (ESM, Node engine floor, optional runner SDKs)', () => {
    const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
    expect(pkg.type).toBe('module');
    // Floor set by the locked toolchain, not the plan's original >=20.10:
    // vitest 4's vite 8 requires ^20.19.0 || >=22.12.0.
    expect(pkg.engines.node).toBe('>=20.19');
    // The four runner SDKs must stay optional so installing/selecting one
    // runner never requires the other three (PLAN.md D3). Unconditional runtime
    // deps: the classify SDK (@anthropic-ai/sdk), Zod (built-in phase schemas),
    // ajv (validates project-supplied per-phase schema overrides —
    // docs/DESIGN-schema-overrides.md), and Undici (the OpenCode loopback
    // transport whose timeout must remain subordinate to the phase deadline).
    expect(Object.keys(pkg.dependencies).sort()).toEqual([
      '@anthropic-ai/sdk',
      'ajv',
      'undici',
      'zod',
    ]);
    expect(Object.keys(pkg.optionalDependencies).sort()).toEqual([
      '@anthropic-ai/claude-agent-sdk',
      '@earendil-works/pi-coding-agent',
      '@openai/codex-sdk',
      '@opencode-ai/sdk',
    ]);
  });
});

describe('verify quality-gate script — issue #2 acceptance criteria', () => {
  let verifyScript: string;

  beforeAll(() => {
    const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as {
      scripts: Record<string, string>;
    };
    verifyScript = pkg.scripts['verify'] ?? '';
  });

  it('scripts.verify exists and is a non-empty string', () => {
    expect(typeof verifyScript).toBe('string');
    expect(verifyScript.length).toBeGreaterThan(0);
  });

  it('scripts.verify chains all required stages', () => {
    // Each stage must appear as a substring — order checked separately
    for (const stage of [
      'typecheck',
      'lint:env',
      'pack:check',
      'mirror:check',
      'wiki:check',
      'coverage',
      'build',
      'rm -rf dist',
    ]) {
      expect(verifyScript, `missing stage: ${stage}`).toContain(stage);
    }
    // npm run coverage is the test stage form used in package.json — it runs the
    // full suite once with v8 coverage + thresholds (issue #36), replacing the
    // bare `npm test` stage.
    expect(verifyScript).toContain('npm run coverage');
  });

  it('scripts.verify uses && (fail-fast) between stages', () => {
    expect(verifyScript).toContain('&&');
    // Every stage that precedes rm -rf dist must be joined with && so a failure aborts
    expect(verifyScript).toMatch(/typecheck.*&&.*lint:env/s);
  });

  it('scripts.verify runs stages in canonical order', () => {
    const idx = (s: string) => verifyScript.indexOf(s);
    // typecheck → lint:env → pack:check → mirror:check → wiki:check → coverage → build → clean
    expect(idx('typecheck')).toBeLessThan(idx('lint:env'));
    expect(idx('lint:env')).toBeLessThan(idx('pack:check'));
    expect(idx('pack:check')).toBeLessThan(idx('mirror:check'));
    expect(idx('mirror:check')).toBeLessThan(idx('wiki:check'));
    expect(idx('wiki:check')).toBeLessThan(idx('npm run coverage'));
    expect(idx('npm run coverage')).toBeLessThan(idx('npm run build'));
    expect(idx('npm run build')).toBeLessThan(idx('rm -rf dist'));
  });

  it('scripts.verify ends with rm -rf dist (no build artifact left behind)', () => {
    expect(verifyScript.trimEnd()).toMatch(/rm -rf dist$/);
  });

  it('all npm run <stage> references in scripts.verify point to defined npm scripts', () => {
    const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as {
      scripts: Record<string, string>;
    };
    const referenced = [...verifyScript.matchAll(/npm run (\S+)/g)]
      .map((m) => m[1])
      .filter((name): name is string => name !== undefined);
    expect(referenced.length).toBeGreaterThan(0);
    for (const name of referenced) {
      expect(pkg.scripts, `verify references "npm run ${name}" but scripts.${name} is undefined`).toHaveProperty(
        name,
      );
    }
  });
});

describe('README.md Development section — issue #2 documentation AC', () => {
  let devSection: string;

  beforeAll(() => {
    const readme = readFileSync(join(REPO_ROOT, 'adw_sdlc', 'README.md'), 'utf8');
    const devStart = readme.indexOf('## Development');
    // Slice to the next top-level heading (or end of file) to get just this section
    const nextHeading = readme.indexOf('\n## ', devStart + 1);
    devSection = nextHeading === -1 ? readme.slice(devStart) : readme.slice(devStart, nextHeading);
  });

  it('## Development section exists', () => {
    expect(devSection.length).toBeGreaterThan(0);
  });

  it('## Development names npm run verify as the canonical gate', () => {
    expect(devSection).toContain('npm run verify');
    expect(devSection).toMatch(/canonical/i);
  });

  it('## Development shows the ADW_TEST_CMD single-command form', () => {
    expect(devSection).toContain('ADW_TEST_CMD');
    expect(devSection).toContain('ADW_TEST_CMD="npm run verify"');
  });

  it('## Development links to docs/LIVE-RUN-BATCH.md', () => {
    expect(devSection).toContain('LIVE-RUN-BATCH.md');
  });
});
