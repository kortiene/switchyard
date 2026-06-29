import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it } from 'vitest';

// Issue #36: guard the coverage instrumentation so it can't silently regress.
// These tests check the configuration declarations — not the runtime output —
// which is the correct scope: the coverage run itself is validated by `npm run
// verify` (which would be circular to invoke from a test).

describe('coverage instrumentation — package.json invariants (issue #36)', () => {
  let pkg: {
    scripts: Record<string, string>;
    devDependencies: Record<string, string>;
  };

  beforeAll(() => {
    pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
  });

  it('@vitest/coverage-v8 is in devDependencies (not deps/optionalDeps)', () => {
    const ver = pkg.devDependencies['@vitest/coverage-v8'];
    expect(typeof ver).toBe('string');
    expect((ver ?? '').length).toBeGreaterThan(0);
  });

  it('scripts.coverage exists and invokes vitest with --coverage', () => {
    expect(typeof pkg.scripts['coverage']).toBe('string');
    expect(pkg.scripts['coverage']).toContain('--coverage');
  });

  it('scripts.verify references npm run coverage (gates the canonical pipeline)', () => {
    expect(pkg.scripts['verify']).toContain('npm run coverage');
  });

  it('scripts.verify does not reference npm test as the test stage (replaced by coverage)', () => {
    // The verify chain must use `npm run coverage`, not `npm test`, so every
    // gate invocation — including the ADW resolve loop — enforces thresholds.
    const verify = pkg.scripts['verify'] ?? '';
    // `npm test` must not appear as a chain stage; `npm run coverage` replaces it.
    // (npm run coverage contains "npm" so we check the full substring.)
    expect(verify).not.toMatch(/&&\s*npm test\s*(&&|$)/);
  });
});

describe('coverage instrumentation — vitest.config.ts invariants (issue #36)', () => {
  let configSource: string;

  beforeAll(() => {
    configSource = readFileSync(new URL('../vitest.config.ts', import.meta.url), 'utf8');
  });

  it('coverage block is present', () => {
    expect(configSource).toContain('coverage');
  });

  it("provider is set to 'v8'", () => {
    expect(configSource).toContain("provider: 'v8'");
  });

  it('include targets src/**/*.ts so untested files count against thresholds', () => {
    expect(configSource).toContain("include: ['src/**/*.ts']");
  });

  it('all: true is set so brand-new untested source files show up at 0%', () => {
    // Without `all: true` a file with no importer is silently omitted from the
    // report — exactly the "untested new branch would not trip the gate" gap.
    expect(configSource).toContain('all: true');
  });

  it('thresholds block is declared', () => {
    expect(configSource).toContain('thresholds');
  });

  it('all four per-metric threshold fields are present (statements, branches, functions, lines)', () => {
    // Prevents partial threshold configs that leave some metrics ungated.
    expect(configSource).toContain('statements:');
    expect(configSource).toContain('branches:');
    expect(configSource).toContain('functions:');
    expect(configSource).toContain('lines:');
  });

  it('autoUpdate is false so the floor cannot silently regress', () => {
    expect(configSource).toContain('autoUpdate: false');
  });

  it('exclude list removes the barrel file (src/index.ts) so re-exports do not pad metrics', () => {
    // src/index.ts is a pure re-export barrel — it would show 100% just from
    // being imported, inflating the global averages. Keep it in exclude.
    expect(configSource).toContain("'src/index.ts'");
  });

  it("reportsDirectory is './coverage' matching the root .gitignore entry", () => {
    // The .gitignore entry is `coverage/`; the reporter writes to `reportsDirectory`.
    // If these diverge the output directory would be committed accidentally.
    expect(configSource).toContain("reportsDirectory: './coverage'");
  });

  it('coverage.enabled is not set to true (focused dev runs must not enforce thresholds)', () => {
    // `enabled` must stay at its default (false): only `vitest run --coverage`
    // collects and enforces. A focused run like `npx vitest run <file>` must
    // not suddenly fail on threshold constraints.
    expect(configSource).not.toContain('enabled: true');
  });
});

describe('.gitignore excludes coverage output (issue #36)', () => {
  let gitignore: string;

  beforeAll(() => {
    // Navigate up two levels from adw_sdlc/ to reach the repo root .gitignore.
    gitignore = readFileSync(new URL('../../.gitignore', import.meta.url), 'utf8');
  });

  it('coverage/ is listed in .gitignore so reports are never accidentally committed', () => {
    expect(gitignore).toContain('coverage/');
  });
});

describe('CI workflow — .github/workflows/verify.yml (issue #36)', () => {
  let workflowSource: string;

  beforeAll(() => {
    workflowSource = readFileSync(new URL('../../.github/workflows/verify.yml', import.meta.url), 'utf8');
  });

  it('verify.yml runs npm run verify (coverage stage is part of that chain)', () => {
    // CI does not invoke coverage directly; it runs `npm run verify` which now
    // includes the `npm run coverage` stage. This test guards end-to-end wiring.
    expect(workflowSource).toContain('npm run verify');
  });

  it('verify.yml sets working-directory to adw_sdlc so the coverage script resolves', () => {
    expect(workflowSource).toContain('working-directory: adw_sdlc');
  });
});
