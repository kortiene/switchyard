/**
 * Live-oriented secret-boundary audit (PLAN.md Section 10, "highest severity";
 * docs/SECRET-BOUNDARY-AUDIT.md). This is the spawn-crossing complement to
 * env.test.ts: where env.test.ts inspects the in-process allowlist *object*,
 * this audit spawns a REAL child with that object as its `env` and reads the
 * child's own `process.env`, proving the spawn actually REPLACES (does not
 * merge) the parent environment so a runner grandchild cannot see a secret.
 *
 * The no-secret-printing rule is load-bearing and structural, not a promise:
 *   1. The source env is seeded with SENTINEL values, never real credentials,
 *      so even a hypothetical value-leaking bug could only ever expose a
 *      sentinel.
 *   2. The child emits the NAMES of any denied keys it sees (expected: none) —
 *      it never echoes process.env wholesale and never prints a value.
 * The deny prefixes are imported from ../src/env.js (single source of truth),
 * never re-hardcoded, so the audit tracks env.ts automatically.
 */

import { spawnSync } from 'node:child_process';
import {
  constants,
  accessSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { ENV_DENY_PREFIXES, RUNNER_ENV_ALLOW, safeSubprocessEnv } from '../src/env.js';
import { RUNNER_IDS } from '../src/invoker.js';

const SENTINEL = 'SENTINEL-DENIED-MUST-NOT-APPEAR';

// Sentinel-valued poisoned source: if a value ever leaked it would expose only
// a sentinel, never a credential. Mirrors the SHAPE of env.test.ts's POISONED.
const POISONED: Record<string, string> = {
  HOME: '/home/u',
  USER: 'u',
  PATH: process.env['PATH'] ?? '/usr/bin',
  GH_TOKEN: SENTINEL,
  GH_BIN: SENTINEL,
  MATRIX_TOKEN: SENTINEL,
  MX_AGENT_FOO: SENTINEL,
  ADW_FOO: SENTINEL,
};

// GitHub keys are denied in PHASED mode (allowGhToken: false) — the env real
// ADW runs use (orchestrator.ts builds the runner child with allowGhToken:
// false). They are intentionally PRESENT in one-shot mode (see test 4).
const DENIED_SPECIFIC = ['GH_TOKEN', 'GH_BIN'] as const;

// The denied keys we expect a no-allowlist (poisoned) spawn to expose: the
// prefix matches in POISONED plus the specific GitHub keys, sorted.
const EXPECTED_LEAK = ['ADW_FOO', 'GH_BIN', 'GH_TOKEN', 'MATRIX_TOKEN', 'MX_AGENT_FOO'];

// Child program: reads its OWN process.env and writes ONLY the NAMES of denied
// keys it can see (JSON array, sorted). The deny list is injected from the
// imported constants, so the child never re-hardcodes the boundary.
function childProgram(prefixes: readonly string[], specific: readonly string[]): string {
  return (
    `const P=${JSON.stringify(prefixes)},S=${JSON.stringify(specific)};` +
    `const hits=Object.keys(process.env).filter(k=>P.some(p=>k.startsWith(p))||S.includes(k));` +
    `process.stdout.write(JSON.stringify(hits.sort()));`
  );
}

const CHILD = childProgram([...ENV_DENY_PREFIXES], [...DENIED_SPECIFIC]);

// Spawn a real child with `env` and return its raw stdout (names only, never
// values — the child program is the single thing that writes to stdout).
function childStdout(env: Record<string, string>): string {
  const r = spawnSync(process.execPath, ['-e', CHILD], { env, encoding: 'utf8' });
  expect(r.status, r.stderr).toBe(0);
  return r.stdout;
}

// The NAMES of denied keys the spawned child can see in its own process.env.
function deniedSeenByChild(env: Record<string, string>): string[] {
  return JSON.parse(childStdout(env)) as string[];
}

describe('live secret-boundary audit (spawned child)', () => {
  it('the phased runner env exposes no denied key to a spawned child, per runner', () => {
    for (const runner of RUNNER_IDS) {
      const env = safeSubprocessEnv({ allowGhToken: false, runner, source: POISONED });
      const leaked = deniedSeenByChild(env);
      // On failure the message lists NAMES only — never a value (AC #1, #2).
      expect(leaked, `${runner} leaked denied keys: ${leaked.join(', ')}`).toEqual([]);
    }
  });

  it('positive control: a no-allowlist spawn DOES expose the denied keys', () => {
    // Spawn the poisoned source directly (no safeSubprocessEnv). If the child
    // could not read its env, or the filter never matched, this would be []
    // and test 1 would be trivially true. It must observe the leak.
    expect(deniedSeenByChild(POISONED)).toEqual(EXPECTED_LEAK);
  });

  it('never prints a secret VALUE — the report is key names only', () => {
    // Even when the child DOES report denied keys (positive control), its
    // stdout must contain key NAMES only, never the sentinel value. This
    // captures the no-secret-printing rule structurally (AC #2).
    const out = childStdout(POISONED);
    expect(out).not.toContain(SENTINEL);
    expect(JSON.parse(out)).toEqual(EXPECTED_LEAK);
  });

  it('one-shot mode (allowGhToken: true) lets GH_TOKEN/GH_BIN through but no deny-prefixed key', () => {
    // Pins the mode-conditionality: "GH_TOKEN absent" is a statement about
    // PHASED mode, not a universal invariant. One-shot intentionally forwards
    // the GitHub keys, but the deny prefixes hold in every mode.
    const env = safeSubprocessEnv({ allowGhToken: true, runner: 'claude', source: POISONED });
    const seen = deniedSeenByChild(env);
    expect(seen).toEqual(['GH_BIN', 'GH_TOKEN']);
    for (const key of seen) {
      for (const prefix of ENV_DENY_PREFIXES) {
        expect(key.startsWith(prefix), key).toBe(false);
      }
    }
  });

  it('EXPECTED_LEAK constant is consistent with POISONED fixture (no stale hardcode)', () => {
    // Guards against POISONED gaining a new deny-prefixed key without updating
    // EXPECTED_LEAK, which would make the positive-control test vacuous.
    const derived = Object.keys(POISONED)
      .filter(
        (k) =>
          [...ENV_DENY_PREFIXES].some((p) => k.startsWith(p)) ||
          [...DENIED_SPECIFIC].includes(k as (typeof DENIED_SPECIFIC)[number]),
      )
      .sort();
    expect(EXPECTED_LEAK).toEqual(derived);
  });

  it('deny-prefixed keys requested via extraAllow are excluded from the spawned child env', () => {
    // Extends env.test.ts "silently drops deny-prefixed keys requested via extraAllow"
    // to the spawn boundary: the env object handed to spawnSync must not contain
    // any key that matches a deny prefix, even if explicitly requested.
    const source = { ...POISONED, ADW_EXTRA: SENTINEL, MATRIX_X: SENTINEL };
    const env = safeSubprocessEnv({
      allowGhToken: false,
      runner: 'claude',
      extraAllow: ['ADW_EXTRA', 'MATRIX_X'],
      source,
    });
    const leaked = deniedSeenByChild(env);
    expect(leaked).toEqual([]);
  });

  it('ENV_DENY_PREFIXES contains exactly the documented deny prefixes (regression guard)', () => {
    // Pin the source-of-truth set so any change to the deny list is noticed and
    // reviewed. Deny-prefix changes are security-relevant and must not slip in
    // as incidental refactors.
    expect([...ENV_DENY_PREFIXES]).toEqual(['MATRIX_', 'MX_AGENT_', 'ADW_']);
  });

  it('allowlisted keys are actually present in the spawned child env (no trivially-empty-env false-pass)', () => {
    // Belt-and-suspenders: if safeSubprocessEnv returned {} the deny-absent
    // check in test 1 would trivially pass. Confirm that base allowlist keys
    // present in POISONED ARE observable in the child, so a clean result is the
    // allowlist working, not an empty env.
    const env = safeSubprocessEnv({ allowGhToken: false, runner: 'claude', source: POISONED });
    const EXPECTED_PRESENT = ['HOME', 'PATH', 'USER'];
    const findAllowed =
      `const A=${JSON.stringify(EXPECTED_PRESENT)};` +
      `const found=A.filter(k=>Object.prototype.hasOwnProperty.call(process.env,k));` +
      `process.stdout.write(JSON.stringify(found.sort()));`;
    const r = spawnSync(process.execPath, ['-e', findAllowed], { env, encoding: 'utf8' });
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toEqual([...EXPECTED_PRESENT].sort());
  });

  it('RUNNER_IDS and RUNNER_ENV_ALLOW cover the same runner set (no audit blind spot)', () => {
    // The per-runner spawn loop iterates RUNNER_IDS; RUNNER_ENV_ALLOW is the
    // per-runner credential table. If a runner is added to one but not the
    // other the audit silently skips or crashes. Pin both sets equal so new
    // runners are caught at review time, not silently.
    expect([...RUNNER_IDS].sort()).toEqual(Object.keys(RUNNER_ENV_ALLOW).sort());
  });

  it('non-denied extraAllow keys actually reach the spawned child env (spawn-boundary forwarding)', () => {
    // Complements env.test.ts "explicit, non-denied extra is honored" at the
    // spawn boundary: the forwarded key must be observable in the child's own
    // process.env, not just present in the in-process env object. Reports
    // presence/absence only — never the value — to stay consistent with the
    // no-secret-printing rule.
    const source = { ...POISONED, CUSTOM_TOOL_BIN: '/usr/local/bin/tool' };
    const env = safeSubprocessEnv({
      allowGhToken: false,
      runner: 'claude',
      extraAllow: ['CUSTOM_TOOL_BIN'],
      source,
    });
    const findKey =
      `const present=Object.prototype.hasOwnProperty.call(process.env,'CUSTOM_TOOL_BIN');` +
      `process.stdout.write(present?'present':'absent');`;
    const r = spawnSync(process.execPath, ['-e', findKey], { env, encoding: 'utf8' });
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toBe('present');
    // Confirm no denied keys slipped in alongside.
    expect(deniedSeenByChild(env)).toEqual([]);
  });

  it('the real-Claude exec probe reports names only and fails closed before exec on a denied key', () => {
    const root = mkdtempSync(join(tmpdir(), 'adw-claude-env-probe-'));
    const clean = join(root, 'clean');
    const denied = join(root, 'denied');
    mkdirSync(clean);
    mkdirSync(denied);
    const probe = join(import.meta.dirname, '..', 'tools', 'claude-env-audit-wrapper.sh');
    const forwarded = join(root, 'capture-forwarded-argv.sh');
    writeFileSync(
      forwarded,
      `#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' "$@" > "\${TMPDIR}/forwarded-argv.txt"\n`,
      { mode: 0o700 },
    );
    accessSync(probe, constants.X_OK);
    const base = {
      PATH: '/usr/bin:/bin',
      HOME: '/tmp',
      CLAUDE_BIN: probe,
      CLAUDE_CODE_PATH: forwarded,
      CLAUDE_CODE_ENTRYPOINT: 'sdk-ts',
      CLAUDE_AGENT_SDK_VERSION: 'test-version',
    };

    try {
      const cleanRun = spawnSync(
        probe,
        ['--model', 'claude-test', '--max-budget-usd', '0.01'],
        { env: { ...base, TMPDIR: clean }, encoding: 'utf8' },
      );
      expect(cleanRun.status, cleanRun.stderr).toBe(0);
      const cleanEvidence = JSON.parse(
        readFileSync(join(clean, 'claude-runner-env-audit.json'), 'utf8'),
      ) as Record<string, unknown>;
      expect(cleanEvidence).toMatchObject({
        value_output_forbidden: true,
        observed_denied_key_names: [],
        requested_model: 'claude-test',
        requested_max_budget_usd: '0.01',
        result: 'PASS',
      });
      expect(cleanEvidence['observed_control_key_names']).toEqual(
        expect.arrayContaining(['CLAUDE_CODE_ENTRYPOINT', 'CLAUDE_AGENT_SDK_VERSION']),
      );
      expect(readFileSync(join(clean, 'forwarded-argv.txt'), 'utf8').split('\n')).toEqual([
        '--model',
        'claude-test',
        '--max-budget-usd',
        '0.01',
        '',
      ]);

      const deniedRun = spawnSync(probe, ['--model', 'claude-test'], {
        env: { ...base, TMPDIR: denied, GH_TOKEN: SENTINEL },
        encoding: 'utf8',
      });
      expect(deniedRun.status).toBe(97);
      const deniedEvidence = JSON.parse(
        readFileSync(join(denied, 'claude-runner-env-audit.json'), 'utf8'),
      ) as Record<string, unknown>;
      expect(deniedEvidence).toMatchObject({
        value_output_forbidden: true,
        observed_denied_key_names: ['GH_TOKEN'],
        result: 'FAIL',
      });
      expect(`${deniedRun.stdout}${deniedRun.stderr}`).not.toContain(SENTINEL);
      expect(JSON.stringify(deniedEvidence)).not.toContain(SENTINEL);
      expect(() => readFileSync(join(denied, 'forwarded-argv.txt'), 'utf8')).toThrow();
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
