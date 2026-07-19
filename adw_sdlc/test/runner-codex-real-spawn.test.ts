/**
 * Real-process names/metadata-only proof for issue #75.
 *
 * Unlike runner-codex-spawn.test.ts, this does not mock child_process.spawn:
 * the real SDK spawns the ADW security launcher, which in turn spawns a
 * hermetic metadata fixture.  No network, credentials, or env values leave
 * the process; the final JSON records only argv and environment variable
 * names at the executable boundary.
 */

import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { safeSubprocessEnv } from '../src/env.js';
import type { PhaseRequest } from '../src/invoker.js';
import { createRunner } from '../src/runners/runner-codex.js';

const metadataProbe = fileURLToPath(
  new URL('./fixtures/codex-metadata-probe.mjs', import.meta.url),
);

let tmp: string;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), 'adw-codex-real-spawn-'));
  vi.stubEnv('GH_TOKEN', 'poison-gh');
  vi.stubEnv('ADW_SECRET', 'poison-adw');
  vi.stubEnv('MATRIX_TOKEN', 'poison-matrix');
  vi.stubEnv('MX_AGENT_SECRET', 'poison-agent');
});

afterEach(() => {
  vi.unstubAllEnvs();
  rmSync(tmp, { recursive: true, force: true });
});

it('preserves durable auth while excluding user config, apps, and plugins at the real spawn boundary', async () => {
  const operatorHome = join(tmp, 'home');
  const codexHome = join(tmp, 'codex-home');
  mkdirSync(operatorHome, { recursive: true });
  mkdirSync(codexHome, { recursive: true });
  const authPath = join(codexHome, 'auth.json');
  const authMarker = '{"marker":"must-remain-in-place"}\n';
  writeFileSync(authPath, authMarker, 'utf8');
  writeFileSync(
    join(codexHome, 'config.toml'),
    '[mcp_servers.forge]\ncommand = "must-not-load"\n',
    'utf8',
  );

  const env = safeSubprocessEnv({
    allowGhToken: false,
    runner: 'codex',
    source: {
      HOME: operatorHome,
      PATH: process.env['PATH'],
      CODEX_HOME: codexHome,
      CODEX_BIN: metadataProbe,
      GH_TOKEN: 'poison-gh',
      ADW_SECRET: 'poison-adw',
      MATRIX_TOKEN: 'poison-matrix',
      MX_AGENT_SECRET: 'poison-agent',
    },
  });
  const req: PhaseRequest = {
    phase: 'plan',
    prompt: 'report boundary metadata',
    model: 'gpt-5.5',
    cwd: tmp,
    env,
    transcriptPath: join(tmp, 'transcript.log'),
    signal: new AbortController().signal,
  };

  const result = await createRunner().runPhase(req);

  expect(result.ok).toBe(true);
  expect(readFileSync(authPath, 'utf8')).toBe(authMarker);
  expect(result.structured).not.toBeNull();
  const argv = result.structured!['argv'] as string[];
  const envNames = result.structured!['env_names'] as string[];
  const configOverrides = argv.flatMap((arg, index) =>
    argv[index - 1] === '--config' ? [arg] : [],
  );

  expect(argv[0]).toBe('exec');
  expect(argv).toContain('--ignore-user-config');
  expect(argv).toContain('--strict-config');
  expect(argv).toContain('--ephemeral');
  expect(configOverrides).toEqual(expect.arrayContaining([
    'features.apps=false',
    'features.hooks=false',
    'features.plugins=false',
    'features.plugin_sharing=false',
    'features.remote_plugin=false',
    'features.skill_mcp_dependency_install=false',
    'features.tool_call_mcp_elicitation=false',
    'features.tool_suggest=false',
    'apps._default.enabled=false',
    'apps._default.destructive_enabled=false',
    'apps._default.open_world_enabled=false',
  ]));

  expect(envNames).toEqual(expect.arrayContaining([
    'CODEX_HOME',
    'CODEX_INTERNAL_ORIGINATOR_OVERRIDE',
    'HOME',
    'PATH',
  ]));
  expect(envNames).not.toContain('CODEX_BIN');
  expect(envNames).not.toContain('GH_TOKEN');
  expect(envNames.some((name) => name.startsWith('ADW_'))).toBe(false);
  expect(envNames.some((name) => name.startsWith('MATRIX_'))).toBe(false);
  expect(envNames.some((name) => name.startsWith('MX_AGENT_'))).toBe(false);
});
