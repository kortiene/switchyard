#!/usr/bin/env node

/**
 * Security launcher for unattended ADW Codex phases.
 *
 * The TypeScript SDK does not expose Codex CLI's `--ignore-user-config`,
 * `--strict-config`, or `--ephemeral` switches.  Keep this shim deliberately
 * small: it inserts those supported CLI flags, then delegates to either the
 * allowlisted CODEX_BIN override or the lockstep @openai/codex package.
 */

import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
if (args[0] !== 'exec') {
  console.error('adw Codex launcher only supports `codex exec`');
  process.exit(2);
}

// Auth continues to use HOME/CODEX_HOME, but config.toml (which can enable
// MCP servers and plugins) is not loaded.  Ephemeral mode also prevents the
// unattended phase from adding session state to that authentication home.
args.splice(1, 0, '--ignore-user-config', '--strict-config', '--ephemeral');

const require = createRequire(import.meta.url);
const launcherPath = fileURLToPath(import.meta.url);
const override = process.env['CODEX_BIN'];
if (override === launcherPath) {
  console.error('CODEX_BIN must not point to the ADW Codex launcher itself');
  process.exit(2);
}
const target = override && override !== ''
  ? override
  : require.resolve('@openai/codex/bin/codex.js');

// CODEX_BIN selects the executable at this boundary; the real CLI does not
// need to inherit it.  Every other name is already the runner's allowlisted
// environment plus the SDK originator marker.
const childEnv = { ...process.env };
delete childEnv['CODEX_BIN'];

const child = spawn(target, args, { env: childEnv, stdio: 'inherit' });

for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(signal, () => {
    if (!child.killed) child.kill(signal);
  });
}

const result = await new Promise((resolve) => {
  child.once('error', (error) => {
    console.error(error);
    resolve({ code: 1, signal: null });
  });
  child.once('exit', (code, signal) => resolve({ code, signal }));
});

if (result.signal) {
  process.kill(process.pid, result.signal);
} else {
  process.exit(result.code ?? 1);
}
