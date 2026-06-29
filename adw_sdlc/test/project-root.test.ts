/**
 * Focused unit and integration tests for the explicit external project root
 * feature (issue #56). Covers ACs 1-11 from the spec (section 6):
 *
 *  AC1/AC2  dry-run loads target config and prints its test gate
 *  AC3      agent phase cwd = project root when set
 *  AC4      agents/ state written under project root
 *  AC5      backward-compat: unset → REPO_ROOT / process.cwd() as before
 *  AC6      path traversal and non-directory roots fail closed
 *  AC7      git/gh/gate cwd = project root when set; unset = process.cwd()
 *  AC8      prompt/schema resolution falls back to the package root
 *  AC9      ADW_PROJECT_ROOT withheld from runner children
 *  AC10     flag > env > default precedence; relative resolves against cwd
 *  AC11     npm run verify stays green (covered by the CI gate, not here)
 *
 * Every test resets setProjectRoot(null) / setAgentsDir(null) /
 * setAdwConfigForTests(null) in afterEach to prevent global state leaks.
 *
 * macOS note: mkdtempSync returns /var/folders/... but realpathSync resolves
 * the symlink to /private/var/folders/.... resolveProjectRoot calls
 * realpathSync internally, so all projectRoot comparisons use realpath(tmp),
 * computed via realpathTmp() helper below.
 */

import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { parseCliArgs } from '../src/cli.js';
import { REPO_ROOT, commandCwd, projectRoot, resolveProjectRoot, setProjectRoot } from '../src/common.js';
import {
  adwConfigPath,
  getAdwConfig,
  parseAdwConfig,
  setAdwConfigForTests,
} from '../src/config.js';
import { AdwError } from '../src/errors.js';
import { capture } from '../src/exec.js';
import { ENV_DENY_PREFIXES, safeSubprocessEnv } from '../src/env.js';
import { ENV_ALIASES, readEnvAlias } from '../src/env-vars.js';
import { run } from '../src/orchestrator.js';
import { DEFAULT_PHASES, templatePath, validatePhaseChain } from '../src/phases.js';
import { runAgentPhase } from '../src/run-phase.js';
import { createMockRunner } from '../src/runners/runner-mock.js';
import { AdwState, agentsDir, setAgentsDir } from '../src/state.js';

// ---------------------------------------------------------------------------
// Shared cleanup — reset all process-global overrides after every test.
// ---------------------------------------------------------------------------

afterEach(() => {
  setProjectRoot(null);
  setAgentsDir(null);
  setAdwConfigForTests(null);
});

// On macOS, /tmp → /private/tmp via a symlink. realpathSync canonicalizes the
// path, so projectRoot() always returns the /private/… form. Comparing against
// the pre-realpath mkdtempSync path fails. This helper resolves the canonical
// form so all assertions can use the same basis.
function realpath(p: string): string {
  return realpathSync(p);
}

// ---------------------------------------------------------------------------
// 1. resolveProjectRoot — validation (AC6)
// ---------------------------------------------------------------------------

describe('resolveProjectRoot — validation (AC6)', () => {
  let tmp: string;

  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), 'adw-proot-'));
  });

  afterEach(() => {
    rmSync(tmp, { recursive: true, force: true });
  });

  it('accepts an existing directory and returns a canonical absolute path', () => {
    const result = resolveProjectRoot(tmp);
    expect(result).toBe(realpath(tmp)); // realpathSync may resolve symlinks (e.g. macOS /var → /private/var)
    expect(result.startsWith('/')).toBe(true);
  });

  it('rejects a non-existent path with an actionable AdwError (includes the path)', () => {
    const missing = join(tmp, 'ghost-dir');
    expect(() => resolveProjectRoot(missing)).toThrow(AdwError);
    expect(() => resolveProjectRoot(missing)).toThrow(/project root does not exist/);
    expect(() => resolveProjectRoot(missing)).toThrow(missing);
  });

  it('rejects a regular file (not a directory) with an actionable AdwError', () => {
    const file = join(tmp, 'regular-file.txt');
    writeFileSync(file, 'not a directory');
    expect(() => resolveProjectRoot(file)).toThrow(AdwError);
    expect(() => resolveProjectRoot(file)).toThrow(/project root is not a directory/);
  });

  it('resolves a relative path against process.cwd() (AC10)', () => {
    // '.' resolves to the invocation cwd (adw_sdlc/ when running `npm test`)
    const result = resolveProjectRoot('.');
    expect(result).toBe(realpath(process.cwd()));
  });

  it('canonicalizes a dot-dot path pointing at a real directory', () => {
    const sub = join(tmp, 'sub');
    mkdirSync(sub);
    // sub/../sub collapses back to sub
    const dotted = join(sub, '..', 'sub');
    expect(resolveProjectRoot(dotted)).toBe(realpath(sub));
  });

  it('fails a dot-dot path leading to a non-existent directory', () => {
    const missing = join(tmp, 'nonexistent', '..', 'ghost');
    expect(() => resolveProjectRoot(missing)).toThrow(/project root does not exist/);
  });
});

// ---------------------------------------------------------------------------
// 2. projectRoot / commandCwd — defaults and override (AC5)
// ---------------------------------------------------------------------------

describe('projectRoot / commandCwd — defaults and override (AC5)', () => {
  let tmp: string;

  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), 'adw-proot2-'));
  });

  afterEach(() => {
    rmSync(tmp, { recursive: true, force: true });
  });

  it('default: projectRoot() === REPO_ROOT, commandCwd() === undefined (AC5)', () => {
    expect(projectRoot()).toBe(REPO_ROOT);
    expect(commandCwd()).toBeUndefined();
  });

  it('after setProjectRoot(tmp) both accessors return the canonical tmp path', () => {
    setProjectRoot(tmp);
    expect(projectRoot()).toBe(realpath(tmp));
    expect(commandCwd()).toBe(realpath(tmp));
  });

  it('setProjectRoot(null) restores defaults (AC5)', () => {
    setProjectRoot(tmp);
    setProjectRoot(null);
    expect(projectRoot()).toBe(REPO_ROOT);
    expect(commandCwd()).toBeUndefined();
  });

  it('setProjectRoot rejects a missing directory before updating the override', () => {
    const missing = join(tmp, 'no-such');
    expect(() => setProjectRoot(missing)).toThrow(AdwError);
    // The override must not have been changed on failure
    expect(projectRoot()).toBe(REPO_ROOT);
    expect(commandCwd()).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// 3. getAdwConfig / adwConfigPath — project root (AC1/AC2/D4)
// ---------------------------------------------------------------------------

describe('getAdwConfig / adwConfigPath — project root (AC1/AC2/D4)', () => {
  let tmp: string;
  let rtmp: string; // realpath form (macOS symlink-safe)

  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), 'adw-cfg-'));
    rtmp = realpath(tmp);
  });

  afterEach(() => {
    rmSync(tmp, { recursive: true, force: true });
  });

  function writeTargetConfig(dir: string, testCmd: string): void {
    mkdirSync(join(dir, '.adw'));
    writeFileSync(
      join(dir, '.adw', 'config.json'),
      JSON.stringify({
        version: 1,
        project: { id: 'ext-proj', name: 'External Project' },
        commands: { defaultTestCommand: testCmd },
      }),
    );
  }

  it('adwConfigPath() returns a path inside the canonical project root when set', () => {
    setProjectRoot(tmp);
    expect(adwConfigPath()).toBe(join(rtmp, '.adw', 'config.json'));
  });

  it('adwConfigPath() defaults to REPO_ROOT/.adw/config.json when unset (AC5)', () => {
    expect(adwConfigPath()).toBe(join(REPO_ROOT, '.adw', 'config.json'));
  });

  it('getAdwConfig() loads the target repo config after setProjectRoot() (AC1/AC2)', () => {
    writeTargetConfig(tmp, 'scripts/verify.sh');
    setProjectRoot(tmp);
    const cfg = getAdwConfig();
    expect(cfg.commands.defaultTestCommand).toBe('scripts/verify.sh');
    expect(cfg.project.id).toBe('ext-proj');
  });

  it('root-aware cache reloads when project root changes (D4)', () => {
    // Warm the cache at the package root first.
    const pkg = getAdwConfig();
    expect(pkg.project.id).toBe('switchyard'); // the committed Switchyard config

    writeTargetConfig(tmp, 'make test');
    setProjectRoot(tmp);
    const ext = getAdwConfig();
    expect(ext.commands.defaultTestCommand).toBe('make test');
    expect(ext).not.toBe(pkg); // different object, cache was invalidated

    // Switching back returns to the package config.
    setProjectRoot(null);
    const back = getAdwConfig();
    expect(back.project.id).toBe('switchyard');
    expect(back.commands.defaultTestCommand).not.toBe('make test');
  });

  it('setAdwConfigForTests override short-circuits even when a project root is set (existing contract preserved)', () => {
    writeTargetConfig(tmp, 'scripts/verify.sh');
    setProjectRoot(tmp);
    const override = parseAdwConfig({ commands: { defaultTestCommand: 'forced-gate' } });
    setAdwConfigForTests(override);
    expect(getAdwConfig().commands.defaultTestCommand).toBe('forced-gate');
  });
});

// ---------------------------------------------------------------------------
// 4. agentsDir — follows the project root (AC4)
// ---------------------------------------------------------------------------

describe('agentsDir — project root (AC4)', () => {
  let tmp: string;
  let rtmp: string;

  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), 'adw-agents-'));
    rtmp = realpath(tmp);
  });

  afterEach(() => {
    rmSync(tmp, { recursive: true, force: true });
  });

  it('default: agentsDir() === join(REPO_ROOT, "agents") (AC5)', () => {
    expect(agentsDir()).toBe(join(REPO_ROOT, 'agents'));
  });

  it('follows the canonical project root when set (AC4)', () => {
    setProjectRoot(tmp);
    expect(agentsDir()).toBe(join(rtmp, 'agents'));
  });

  it('explicit setAgentsDir override beats the project root (AC4 "unless explicitly overridden")', () => {
    const explicit = join(tmp, 'my-agents');
    setProjectRoot(tmp);
    setAgentsDir(explicit);
    expect(agentsDir()).toBe(explicit);
  });

  it('clearing both restores the default (AC5)', () => {
    setProjectRoot(tmp);
    setAgentsDir(join(tmp, 'x'));
    setProjectRoot(null);
    setAgentsDir(null);
    expect(agentsDir()).toBe(join(REPO_ROOT, 'agents'));
  });
});

// ---------------------------------------------------------------------------
// 5. runAgentPhase — agent cwd (AC3/AC5)
// ---------------------------------------------------------------------------

describe('runAgentPhase — agent cwd (AC3/AC5)', () => {
  let stateDir: string;
  let projDir: string;
  let rprojDir: string;
  let state: AdwState;

  beforeEach(() => {
    stateDir = mkdtempSync(join(tmpdir(), 'adw-rp-state-'));
    projDir = mkdtempSync(join(tmpdir(), 'adw-rp-proj-'));
    rprojDir = realpath(projDir);
    setAgentsDir(stateDir);
    state = new AdwState({ adwId: 'a1b2c3d4' });
  });

  afterEach(() => {
    rmSync(stateDir, { recursive: true, force: true });
    rmSync(projDir, { recursive: true, force: true });
  });

  it('defaults to REPO_ROOT as cwd when no project root is set (AC5)', async () => {
    const seenCwds: string[] = [];
    const runner = createMockRunner({
      script: (req) => {
        seenCwds.push(req.cwd);
        return { structured: { issue_class: 'feat', reason: 'r' } };
      },
    });
    await runAgentPhase({ phase: 'classify', templateArgs: ['5', 'ctx'], state, runner, env: {} });
    expect(seenCwds[0]).toBe(REPO_ROOT);
  });

  it('uses the canonical project root as cwd when set (AC3)', async () => {
    setProjectRoot(projDir);
    const seenCwds: string[] = [];
    const runner = createMockRunner({
      script: (req) => {
        seenCwds.push(req.cwd);
        return { structured: { issue_class: 'feat', reason: 'r' } };
      },
    });
    await runAgentPhase({ phase: 'classify', templateArgs: ['5', 'ctx'], state, runner, env: {} });
    expect(seenCwds[0]).toBe(rprojDir);
  });

  it('an explicit options.cwd wins over the project root (AC3)', async () => {
    const overrideDir = mkdtempSync(join(tmpdir(), 'adw-rp-override-'));
    try {
      setProjectRoot(projDir);
      const seenCwds: string[] = [];
      const runner = createMockRunner({
        script: (req) => {
          seenCwds.push(req.cwd);
          return { structured: { issue_class: 'feat', reason: 'r' } };
        },
      });
      await runAgentPhase({
        phase: 'classify',
        templateArgs: ['5', 'ctx'],
        state,
        runner,
        env: {},
        cwd: overrideDir,
      });
      expect(seenCwds[0]).toBe(overrideDir);
    } finally {
      rmSync(overrideDir, { recursive: true, force: true });
    }
  });
});

// ---------------------------------------------------------------------------
// 6. capture — command cwd routing (AC7/D3)
// ---------------------------------------------------------------------------

describe('capture — command cwd routing (AC7/D3)', () => {
  let tmp: string;
  let rtmp: string;

  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), 'adw-cmd-cwd-'));
    rtmp = realpath(tmp);
  });

  afterEach(() => {
    rmSync(tmp, { recursive: true, force: true });
  });

  it('commandCwd() is undefined when no project root is set (D3)', () => {
    expect(commandCwd()).toBeUndefined();
  });

  it('commandCwd() equals the canonical project root when set (D3)', () => {
    setProjectRoot(tmp);
    expect(commandCwd()).toBe(rtmp);
  });

  it('when project root is set: a subprocess spawned via capture() runs in the project root (AC7)', () => {
    setProjectRoot(tmp);
    const result = capture(['node', '-e', "process.stdout.write(process.cwd())"]);
    expect(result.returncode).toBe(0);
    expect(result.stdout).toBe(rtmp);
  });

  it('when no project root: capture runs in process.cwd(), NOT REPO_ROOT (D3 regression guard)', () => {
    const result = capture(['node', '-e', "process.stdout.write(process.cwd())"]);
    expect(result.returncode).toBe(0);
    // Inherits the test runner's process.cwd() (adw_sdlc/ when running `npm test`).
    expect(result.stdout).toBe(realpath(process.cwd()));
    // The key invariant: the in-repo gate is NOT redirected to REPO_ROOT when unset.
    // (REPO_ROOT is the parent of adw_sdlc/, which has no package.json, so npm
    // gates running in REPO_ROOT would fail to find their scripts.)
    expect(result.stdout).not.toBe(REPO_ROOT);
  });
});

// ---------------------------------------------------------------------------
// 7. templatePath — project root and package-root fallback (AC8/D2)
// ---------------------------------------------------------------------------

describe('templatePath — project root and package-root fallback (AC8/D2)', () => {
  let tmp: string;

  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), 'adw-tpl-'));
  });

  afterEach(() => {
    rmSync(tmp, { recursive: true, force: true });
  });

  function externalConfig() {
    // A config whose prompts.defaultRoot is .adw/prompts (the default).
    return parseAdwConfig({
      version: 1,
      project: { id: 'ext', name: 'Ext' },
    });
  }

  it('falls back to the package root when the project has no .adw/prompts (AC8/D2)', () => {
    // tmp has NO .adw/prompts — templatePath must fall back to REPO_ROOT.
    setProjectRoot(tmp);
    const config = externalConfig();
    const resolved = templatePath('claude', 'plan', config);
    expect(resolved).toBe(join(REPO_ROOT, '.adw', 'prompts', 'plan.md'));
  });

  it('project-local template wins over the package fallback (AC8)', () => {
    setProjectRoot(tmp);
    // Create a project-local plan.md.
    mkdirSync(join(tmp, '.adw', 'prompts'), { recursive: true });
    const localPlan = join(tmp, '.adw', 'prompts', 'plan.md');
    writeFileSync(localPlan, '# project plan\n');
    const config = externalConfig();
    const resolved = templatePath('claude', 'plan', config);
    // Should resolve to the canonical form of the local file.
    expect(resolved).toBe(realpath(localPlan));
  });

  it('validatePhaseChain does NOT throw for a prompt-less external project root (AC8/D2)', () => {
    // This reproduces the §1.4.1 subtlety: without the package-root fallback,
    // --dry-run would throw "missing its prompt template" before printing anything.
    setProjectRoot(tmp);
    const config = externalConfig();
    // Should not throw; all templates resolve via the package-root fallback.
    expect(() => validatePhaseChain([...DEFAULT_PHASES], 'claude', config)).not.toThrow();
  });

  it('in-repo: projectRoot() === REPO_ROOT means both tiers are the same (AC5, fallback is a no-op)', () => {
    // No setProjectRoot call — defaults to REPO_ROOT.
    const config = externalConfig();
    const resolved = templatePath('claude', 'plan', config);
    // The project-tier candidate (REPO_ROOT/.adw/prompts/plan.md) exists and is returned.
    expect(resolved).toBe(join(REPO_ROOT, '.adw', 'prompts', 'plan.md'));
  });
});

// ---------------------------------------------------------------------------
// 8. CLI --project-root flag and ADW_PROJECT_ROOT env (AC10)
// ---------------------------------------------------------------------------

describe('CLI --project-root flag (AC10)', () => {
  it('parseCliArgs passes --project-root into options.projectRoot', () => {
    const parsed = parseCliArgs(['5', '--project-root', '/some/external/repo']);
    expect(parsed.options.projectRoot).toBe('/some/external/repo');
  });

  it('--project-root= (equals form) also works', () => {
    const parsed = parseCliArgs(['5', '--project-root=/opt/target']);
    expect(parsed.options.projectRoot).toBe('/opt/target');
  });

  it('ADW_PROJECT_ROOT env is read when --project-root flag is absent (AC10)', () => {
    const parsed = parseCliArgs(['5'], { ADW_PROJECT_ROOT: '/env/root' });
    expect(parsed.options.projectRoot).toBe('/env/root');
  });

  it('--project-root flag takes precedence over ADW_PROJECT_ROOT env (AC10)', () => {
    const parsed = parseCliArgs(['5', '--project-root', '/flag/root'], {
      ADW_PROJECT_ROOT: '/env/root',
    });
    expect(parsed.options.projectRoot).toBe('/flag/root');
  });

  it('--project-root with no value throws AdwError', () => {
    expect(() => parseCliArgs(['5', '--project-root'])).toThrow(/--project-root requires a value/);
    expect(() => parseCliArgs(['5', '--project-root'])).toThrow(AdwError);
  });

  it('the flag is accepted without throwing (VALUE_FLAGS membership check)', () => {
    expect(() => parseCliArgs(['5', '--project-root', 'x'])).not.toThrow();
  });

  it('when neither flag nor env is set, options.projectRoot is absent (AC5)', () => {
    const parsed = parseCliArgs(['5']);
    expect(parsed.options).not.toHaveProperty('projectRoot');
  });
});

// ---------------------------------------------------------------------------
// 9. End-to-end dry-run with external project root (AC1/AC2)
// ---------------------------------------------------------------------------

describe('dry-run with external project root (AC1/AC2)', () => {
  let tmp: string;
  let rtmp: string;
  let stateDir: string;

  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), 'adw-dr-'));
    rtmp = realpath(tmp);
    stateDir = mkdtempSync(join(tmpdir(), 'adw-dr-state-'));
    setAgentsDir(stateDir);
  });

  afterEach(() => {
    rmSync(tmp, { recursive: true, force: true });
    rmSync(stateDir, { recursive: true, force: true });
  });

  it('loads the target config and prints the correct test gate and project root (AC1/AC2)', async () => {
    // Write a minimal external project .adw/config.json.
    mkdirSync(join(tmp, '.adw'));
    writeFileSync(
      join(tmp, '.adw', 'config.json'),
      JSON.stringify({
        version: 1,
        project: { id: 'iroh-room', name: 'iroh-room' },
        commands: { defaultTestCommand: 'scripts/verify.sh' },
      }),
    );

    const logs: string[] = [];
    const log = vi.spyOn(console, 'log').mockImplementation((...args: unknown[]) => {
      logs.push(args.join(' '));
    });
    try {
      const rc = await run(
        5,
        createMockRunner(),
        {
          dryRun: true,
          projectRoot: tmp,
          allowDirty: true,
          verify: false,
        },
        {},
      );
      expect(rc).toBe(0);
      const output = logs.join('\n');
      // AC2: the target test gate is printed.
      expect(output).toContain('[dry-run] test gate: scripts/verify.sh');
      // AC1: the project root is printed (canonical path set by setProjectRoot).
      expect(output).toContain(`[dry-run] project root: ${rtmp}`);
    } finally {
      log.mockRestore();
    }
  });

  it('backward-compat: dry-run without --project-root shows the package REPO_ROOT (AC5)', async () => {
    const logs: string[] = [];
    const log = vi.spyOn(console, 'log').mockImplementation((...args: unknown[]) => {
      logs.push(args.join(' '));
    });
    try {
      const rc = await run(5, createMockRunner(), { dryRun: true, allowDirty: true, verify: false }, {});
      expect(rc).toBe(0);
      const output = logs.join('\n');
      // REPO_ROOT is the project root in the default case.
      expect(output).toContain(`[dry-run] project root: ${REPO_ROOT}`);
    } finally {
      log.mockRestore();
    }
  });
});

// ---------------------------------------------------------------------------
// 10. ADW_PROJECT_ROOT — secret boundary (AC9)
// ---------------------------------------------------------------------------

describe('ADW_PROJECT_ROOT — secret boundary (AC9)', () => {
  it('ADW_PROJECT_ROOT is covered by the ADW_ deny prefix in ENV_DENY_PREFIXES', () => {
    const matched = ENV_DENY_PREFIXES.some((prefix) => 'ADW_PROJECT_ROOT'.startsWith(prefix));
    expect(matched).toBe(true);
  });

  it('safeSubprocessEnv withholds ADW_PROJECT_ROOT in phased mode (AC9)', () => {
    const source = {
      ADW_PROJECT_ROOT: '/external/repo',
      PATH: '/bin',
      HOME: '/home/u',
      USER: 'u',
    };
    const env = safeSubprocessEnv({ allowGhToken: false, source });
    expect(env).not.toHaveProperty('ADW_PROJECT_ROOT');
  });

  it('ADW_PROJECT_ROOT is withheld even with a runner-specific env (claude runner)', () => {
    const source = {
      ADW_PROJECT_ROOT: '/external/repo',
      ANTHROPIC_API_KEY: 'sk-ant-x',
      PATH: '/bin',
      HOME: '/home/u',
      USER: 'u',
    };
    const env = safeSubprocessEnv({ allowGhToken: false, runner: 'claude', source });
    expect(env).not.toHaveProperty('ADW_PROJECT_ROOT');
    // Credentials still flow through for their runner
    expect(env['ANTHROPIC_API_KEY']).toBe('sk-ant-x');
  });

  it('ADW_PROJECT_ROOT is withheld even in one-shot mode (allowGhToken=true)', () => {
    const source = {
      ADW_PROJECT_ROOT: '/external/repo',
      GH_TOKEN: 'ghp_x',
      PATH: '/bin',
      HOME: '/home/u',
      USER: 'u',
    };
    const env = safeSubprocessEnv({ allowGhToken: true, source });
    expect(env).not.toHaveProperty('ADW_PROJECT_ROOT');
    expect(env['GH_TOKEN']).toBe('ghp_x'); // GH_TOKEN is intentionally present in one-shot mode
  });
});

// ---------------------------------------------------------------------------
// 11. ENV_ALIASES.projectRoot — readEnvAlias behavior (D5)
// ---------------------------------------------------------------------------

describe('ENV_ALIASES.projectRoot — readEnvAlias behavior (D5)', () => {
  it('the ENV_ALIASES table has canonical: ADW_PROJECT_ROOT and legacy: MX_AGENT_PROJECT_ROOT', () => {
    expect(ENV_ALIASES.projectRoot.canonical).toBe('ADW_PROJECT_ROOT');
    expect(ENV_ALIASES.projectRoot.legacy).toBe('MX_AGENT_PROJECT_ROOT');
  });

  it('reads ADW_PROJECT_ROOT as the canonical name', () => {
    expect(readEnvAlias({ ADW_PROJECT_ROOT: '/canonical' }, ENV_ALIASES.projectRoot)).toBe('/canonical');
  });

  it('throws when canonical and legacy values conflict', () => {
    expect(() =>
      readEnvAlias(
        { ADW_PROJECT_ROOT: '/canonical', MX_AGENT_PROJECT_ROOT: '/other' },
        ENV_ALIASES.projectRoot,
      ),
    ).toThrow(AdwError);
    expect(() =>
      readEnvAlias(
        { ADW_PROJECT_ROOT: '/canonical', MX_AGENT_PROJECT_ROOT: '/other' },
        ENV_ALIASES.projectRoot,
      ),
    ).toThrow(/conflicting env vars: ADW_PROJECT_ROOT and deprecated MX_AGENT_PROJECT_ROOT/);
  });

  it('canonical wins when both are set to the same value', () => {
    const result = readEnvAlias(
      { ADW_PROJECT_ROOT: '/same', MX_AGENT_PROJECT_ROOT: '/same' },
      ENV_ALIASES.projectRoot,
    );
    expect(result).toBe('/same');
  });

  it('returns undefined when neither name is in the env', () => {
    expect(readEnvAlias({}, ENV_ALIASES.projectRoot)).toBeUndefined();
    expect(readEnvAlias({ PATH: '/bin' }, ENV_ALIASES.projectRoot)).toBeUndefined();
  });

  it('accepts the legacy MX_AGENT_PROJECT_ROOT alias and returns its value', () => {
    // The warnedLegacy set is a module-level singleton; a prior test invocation
    // for this alias within the same vitest worker may have already consumed
    // the first warning. We verify the VALUE is returned correctly; the warning
    // behavior is transitively covered by the existing env-vars.test.ts suite.
    const result = readEnvAlias({ MX_AGENT_PROJECT_ROOT: '/legacy' }, ENV_ALIASES.projectRoot);
    expect(result).toBe('/legacy');
  });
});
