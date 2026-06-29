/**
 * E2e coverage for issue #2 acceptance criteria that require a real subprocess.
 *
 * scaffold.test.ts covers the static structure (package.json + README content).
 * These tests cover the runtime behavior AC2 targets: the production build
 * (`tsc -p tsconfig.build.json`) actually emits its artifact tree, and the
 * `rm -rf` cleanup actually removes it.
 *
 * De-flaked (#41 follow-up): the build is emitted into a per-test temp `outDir`
 * (mkdtemp) instead of the package-root `dist/`. The shared `dist/` is ALSO
 * produced by the verify chain's own `build` stage and removed by its
 * `rm -rf dist`; sharing that one path across parallel test files raced on
 * `existsSync`. An isolated outDir exercises the same compiler config + the same
 * `rm -rf` cleanup semantics, deterministically and with no shared-path contention.
 *
 * We intentionally do NOT run `npm run verify` here (circular: npm test is a
 * stage inside verify) — only the build + cleanup stages that touch the artifact.
 */

import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

const PKG_DIR = new URL('..', import.meta.url).pathname.replace(/\/$/, '');

let workDir: string;
let outDir: string;

beforeEach(() => {
  // A unique build target per test — no contention with the shared package dist/.
  workDir = mkdtempSync(join(tmpdir(), 'adw-verify-e2e-'));
  outDir = join(workDir, 'dist');
});

afterEach(() => {
  rmSync(workDir, { recursive: true, force: true });
});

describe('verify quality-gate — build artifact lifecycle (e2e)', () => {
  it(
    'tsc build emits the artifact tree and `rm -rf` removes it',
    { timeout: 120_000 },
    () => {
      expect(existsSync(outDir), 'outDir must not exist before build').toBe(false);

      // Same compiler config as `npm run build`, redirected to the isolated outDir.
      const buildResult = spawnSync(
        'npx',
        ['tsc', '-p', 'tsconfig.build.json', '--outDir', outDir],
        { cwd: PKG_DIR, encoding: 'utf8' },
      );
      expect(buildResult.status, `tsc build failed:\n${buildResult.stderr}`).toBe(0);
      expect(existsSync(outDir), 'outDir must exist after build').toBe(true);

      // The exact cleanup mechanism scripts.verify uses (`rm -rf <build dir>`).
      const rmResult = spawnSync('rm', ['-rf', outDir], { cwd: PKG_DIR, encoding: 'utf8' });
      expect(rmResult.status, `rm -rf failed:\n${rmResult.stderr}`).toBe(0);
      expect(existsSync(outDir), 'outDir must be gone after rm -rf (AC2)').toBe(false);
    },
  );

  it(
    'tsc exits non-zero for a broken build (fail-fast signal) and emits nothing',
    { timeout: 60_000 },
    () => {
      // A non-existent tsconfig forces tsc to exit non-zero, confirming the build
      // stage can signal failure so the verify chain's && stops execution.
      const result = spawnSync(
        'npx',
        ['tsc', '-p', 'tsconfig.does-not-exist.json', '--outDir', outDir],
        { cwd: PKG_DIR, encoding: 'utf8' },
      );
      expect(result.status, 'tsc with missing config must exit non-zero').not.toBe(0);
      expect(existsSync(outDir), 'outDir must not be created on a failed build').toBe(false);
    },
  );
});
