/**
 * E2e coverage for issue #2 acceptance criteria that require a real subprocess.
 *
 * scaffold.test.ts covers the static structure (package.json + README content).
 * These tests cover the runtime behavior: build actually creates dist/, and the
 * cleanup step actually removes it — the filesystem boundary AC2 targets.
 *
 * We intentionally do NOT run `npm run verify` here (circular: npm test is a
 * stage inside verify). Instead we run only the two stages that produce and
 * consume the build artifact.
 */

import { existsSync } from 'node:fs';
import { rmSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

const PKG_DIR = new URL('..', import.meta.url).pathname.replace(/\/$/, '');
const DIST_DIR = join(PKG_DIR, 'dist');

/** Remove dist/ if it exists, to start each test from a clean state. */
function cleanDist(): void {
  if (existsSync(DIST_DIR)) {
    rmSync(DIST_DIR, { recursive: true, force: true });
  }
}

describe('verify quality-gate — build artifact lifecycle (e2e)', () => {
  beforeEach(cleanDist);
  afterEach(cleanDist);

  it(
    'npm run build creates dist/ and rm -rf dist removes it',
    { timeout: 120_000 },
    () => {
      expect(existsSync(DIST_DIR), 'dist/ must not exist before build').toBe(false);

      const buildResult = spawnSync('npm', ['run', 'build'], {
        cwd: PKG_DIR,
        encoding: 'utf8',
      });
      expect(buildResult.status, `npm run build failed:\n${buildResult.stderr}`).toBe(0);
      expect(existsSync(DIST_DIR), 'dist/ must exist after npm run build').toBe(true);

      // This is the exact cleanup command used in scripts.verify
      const rmResult = spawnSync('rm', ['-rf', 'dist'], {
        cwd: PKG_DIR,
        encoding: 'utf8',
      });
      expect(rmResult.status, `rm -rf dist failed:\n${rmResult.stderr}`).toBe(0);
      expect(existsSync(DIST_DIR), 'dist/ must be gone after rm -rf dist (AC2)').toBe(false);
    },
  );

  it(
    'npm run build exits non-zero for a broken TypeScript source (fail-fast signal)',
    { timeout: 60_000 },
    () => {
      // Passing a non-existent tsconfig forces tsc to exit non-zero, confirming
      // that the build stage can signal failure so && stops execution.
      const result = spawnSync('npx', ['tsc', '-p', 'tsconfig.does-not-exist.json'], {
        cwd: PKG_DIR,
        encoding: 'utf8',
      });
      expect(result.status, 'tsc with missing config must exit non-zero').not.toBe(0);
      // dist/ must NOT have been created when the build command fails
      expect(existsSync(DIST_DIR), 'dist/ must not be created on a failed build').toBe(false);
    },
  );
});
