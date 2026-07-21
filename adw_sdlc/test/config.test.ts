import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';
import { DEFAULT_ADW_CONFIG, isClosedWorkItemState, loadAdwConfig, parseAdwConfig } from '../src/config.js';
import { formatProgress } from '../src/exec.js';
import { branchPrefix, deriveBranch, slugifyTitle } from '../src/issue.js';
import { classifyModel, modelForPhase } from '../src/models.js';
import { gateDocument, gateE2e, templatePath } from '../src/phases.js';

function tempJson(value: unknown): string {
  const dir = mkdtempSync(join(tmpdir(), 'adw-config-'));
  const path = join(dir, 'config.json');
  writeFileSync(path, JSON.stringify(value), 'utf8');
  return path;
}

describe('ADW config', () => {
  it('loads the committed .adw/config.json project pack', () => {
    const config = loadAdwConfig();
    expect(config.version).toBe(1);
    expect(config.project).toEqual({ id: 'switchyard', name: 'Switchyard' });
    expect(config.prompts.defaultRoot).toBe('.adw/prompts');
    expect(config.prompts.runnerRoots).toEqual({});
    expect(config.progress.tag).toBe('[MX-ADW]');
    expect(config.providers).toEqual(DEFAULT_ADW_CONFIG.providers);
    expect(config.branching.labelPrefixes).toEqual(DEFAULT_ADW_CONFIG.branching.labelPrefixes);
    expect(config.models.phaseTiers).toEqual(DEFAULT_ADW_CONFIG.models.phaseTiers);
  });

  it('falls back to defaults when no config file exists', () => {
    const path = join(mkdtempSync(join(tmpdir(), 'adw-config-missing-')), 'absent.json');
    expect(loadAdwConfig(path)).toEqual(parseAdwConfig({}));
  });

  it('deep-merges partial project config and normalizes label keys', () => {
    const config = parseAdwConfig({
      project: { id: 'payments', name: 'Payments' },
      progress: { tag: '[PAY-ADW]' },
      branching: {
        defaultPrefix: 'task',
        labelPrefixes: { Security: 'fix' },
        slug: { maxLength: 12 },
      },
      gates: {
        e2e: { hints: ['webhook'] },
        documentation: { fileExtensions: ['.adoc'] },
      },
      models: {
        classifyModel: 'cheap-classifier',
        phaseTiers: { implement: 'mid' },
        tiers: { mid: { claude: 'project-sonnet' } },
      },
    });

    expect(config.project.id).toBe('payments');
    expect(config.branching.labelPrefixes['security']).toBe('fix');
    expect(config.branching.labelPrefixes['bug']).toBe('fix'); // default retained
    expect(config.branching.slug.maxLength).toBe(12);
    expect(config.branching.slug.stripDiacritics).toBe(true); // default retained
    expect(config.gates.e2e.hints).toEqual(['webhook']);
    expect(config.gates.documentation.fileExtensions).toEqual(['.adoc']);
    expect(config.models.tiers.mid.claude).toBe('project-sonnet');
    expect(config.models.tiers.mid.codex).toBe(DEFAULT_ADW_CONFIG.models.tiers.mid.codex); // default retained
  });

  it('accepts OpenCode server config plus one explicitly named provider credential', () => {
    const config = parseAdwConfig({
      runners: {
        opencode: {
          authEnv: 'LOCAL_MODEL_API_KEY',
          config: {
            enabled_providers: ['local'],
            small_model: 'local/qwen',
            provider: {
              local: {
                npm: '@ai-sdk/openai-compatible',
                options: { baseURL: 'http://127.0.0.1:8000/v1', apiKey: '{env:LOCAL_MODEL_API_KEY}' },
              },
            },
          },
        },
      },
    });

    expect(config.runners.opencode.authEnv).toBe('LOCAL_MODEL_API_KEY');
    expect(config.runners.opencode.config).toMatchObject({
      enabled_providers: ['local'],
      small_model: 'local/qwen',
      provider: {
        local: {
          options: { baseURL: 'http://127.0.0.1:8000/v1', apiKey: '{env:LOCAL_MODEL_API_KEY}' },
        },
      },
    });
  });

  it('accepts a non-empty list of OpenCode provider credential names', () => {
    const authEnv = ['SAKANA_API_KEY', 'ZAI_API_KEY'];
    const config = parseAdwConfig({ runners: { opencode: { authEnv } } });

    expect(config.runners.opencode.authEnv).toEqual(authEnv);
  });

  it('rejects unsafe OpenCode auth indirection names and non-object server config', () => {
    for (const authEnv of [
      'GH_TOKEN',
      'GITHUB_TOKEN',
      'GH_ENTERPRISE_TOKEN',
      'GITHUB_ENTERPRISE_TOKEN',
      'GH_BIN',
      'ADW_LOCAL_KEY',
      'MX_AGENT_LOCAL_KEY',
      'MATRIX_TOKEN',
      'OPENCODE_CONFIG',
      'OPENCODE_CONFIG_CONTENT',
      'OPENCODE_CONFIG_DIR',
      'CODEX_API_KEY',
      'CLAUDE_CODE_OAUTH_TOKEN',
      'PI_CODING_AGENT_DIR',
      'NOT-A-NAME',
    ]) {
      expect(() => parseAdwConfig({ runners: { opencode: { authEnv } } }), authEnv).toThrow(/invalid/);
      expect(
        () => parseAdwConfig({ runners: { opencode: { authEnv: ['SAFE_PROVIDER_KEY', authEnv] } } }),
        authEnv,
      ).toThrow(/invalid/);
    }
    expect(() => parseAdwConfig({ runners: { opencode: { authEnv: [] } } })).toThrow(/invalid/);
    expect(() => parseAdwConfig({ runners: { opencode: { config: [] } } })).toThrow(/invalid/);
    expect(() => parseAdwConfig({ runners: { opencode: { config: 'provider' } } })).toThrow(/invalid/);
  });

  it('fails loudly for invalid config', () => {
    expect(() => loadAdwConfig(tempJson({ version: 2 }))).toThrow(/invalid/);
    expect(() => parseAdwConfig({ branching: { slug: { maxLength: 0 } } })).toThrow(/maxLength/);
    // Provider type is shape-validated: a blank type is rejected at load.
    expect(() => parseAdwConfig({ providers: { vcs: { type: '' } } })).toThrow(/providers.vcs.type/);
  });

  it('shape-validates provider type, deferring kind membership to the registry', () => {
    // A non-empty unknown type PARSES — membership (which kinds the kernel can
    // actually build) is the provider registry's job, the same shape/membership
    // split the phase chain uses. createProvidersFromConfig fails closed later.
    expect(parseAdwConfig({ providers: { vcs: { type: 'gitlab' } } }).providers.vcs.type).toBe('gitlab');
    expect(parseAdwConfig({ providers: { workItems: { type: 'jira' } } }).providers.workItems.type).toBe('jira');
  });

  it('accepts an optional ordered phase chain, validated for shape only', () => {
    // Absent by default — the kernel then runs its full built-in catalog.
    expect(parseAdwConfig({}).phases).toBeUndefined();
    // A provided chain is preserved verbatim; membership is the kernel's job.
    expect(parseAdwConfig({ phases: ['classify', 'plan', 'implement'] }).phases).toEqual([
      'classify',
      'plan',
      'implement',
    ]);
    // Shape failures are rejected at load: an empty list or a blank entry.
    expect(() => parseAdwConfig({ phases: [] })).toThrow(/invalid/);
    expect(() => parseAdwConfig({ phases: [''] })).toThrow(/invalid/);
  });

  it('accepts optional custom gates and loops, defaulting empty', () => {
    // Absent by default — no custom control flow for the committed config.
    const d = parseAdwConfig({});
    expect(d.gates.custom).toEqual({});
    expect(d.loops).toEqual({});
    // Provided entries fill their own defaults (gate matchers empty, loop maxAttempts 3).
    const c = parseAdwConfig({
      customPhases: ['audit', 'verify'],
      gates: { custom: { audit: { hints: ['payment'] } } },
      loops: { verify: { command: 'npm run verify' } },
    });
    expect(c.gates.custom['audit']).toEqual({
      hints: ['payment'],
      exactFiles: [],
      pathPrefixes: [],
      fileExtensions: [],
    });
    expect(c.loops['verify']).toEqual({ command: 'npm run verify', maxAttempts: 3 });
    // Shape failures: a blank loop command, or a non-positive maxAttempts.
    expect(() => parseAdwConfig({ loops: { verify: { command: '' } } })).toThrow(/invalid/);
    expect(() => parseAdwConfig({ loops: { verify: { command: 'x', maxAttempts: 0 } } })).toThrow(/invalid/);
  });

  it('lets projects state the built-in provider selections explicitly', () => {
    const config = parseAdwConfig({
      providers: {
        cli: { type: 'github' },
        workItems: { type: 'github', closedStates: ['CLOSED'] },
        vcs: { type: 'git' },
        changeRequests: { type: 'github' },
      },
    });
    expect(config.providers).toEqual({
      cli: { type: 'github' },
      workItems: {
        type: 'github',
        closedStates: ['CLOSED'],
        inProgressStatus: 'In Progress',
        statusFieldName: 'Status',
      },
      vcs: { type: 'git' },
      changeRequests: { type: 'github' },
    });
  });
});

describe('config-driven behavior', () => {
  const config = parseAdwConfig({
    progress: { tag: '[ACME-ADW]' },
    branching: {
      defaultPrefix: 'task',
      labelPrefixes: { Security: 'fix' },
      slug: { maxLength: 10, stripPhaseIssuePrefix: false },
    },
    gates: {
      e2e: { hints: ['webhook'] },
      documentation: {
        hints: ['operator guide'],
        exactFiles: ['CHANGELOG.adoc'],
        pathPrefixes: ['guides/'],
        fileExtensions: ['.adoc'],
      },
    },
    models: {
      classifyModel: 'project-classifier',
      phaseTiers: { implement: 'mid' },
      tiers: { mid: { claude: 'project-sonnet' } },
    },
  });

  it('drives branch prefixes and slug options', () => {
    expect(branchPrefix([], config)).toBe('task');
    expect(branchPrefix(['Security'], config)).toBe('fix');
    expect(slugifyTitle('Phase issue 9: Déjà vu branch title', config)).toBe('phase-issu');
    expect(deriveBranch(9, 'Patch prod incident', ['security'], 'a1b2c3d4', config)).toBe(
      'fix/9-a1b2c3d4-patch-prod',
    );
  });

  it('drives conditional gates', () => {
    expect(gateE2e('handle stripe webhook retries', config).runIt).toBe(true);
    expect(gateE2e('touch qr consent flow', config).runIt).toBe(false);
    expect(gateDocument('internal change', ['guides/operator.adoc'], config).runIt).toBe(true);
    expect(gateDocument('update operator guide', ['src/index.ts'], config).runIt).toBe(true);
    expect(gateDocument('add cli flag', ['src/index.ts'], config).runIt).toBe(false);
  });

  it('drives prompt root lookup', () => {
    const root = mkdtempSync(join(tmpdir(), 'adw-prompts-'));
    const claudeRoot = join(root, 'claude');
    const defaultRoot = join(root, 'default');
    mkdirSync(claudeRoot);
    mkdirSync(defaultRoot);
    writeFileSync(join(claudeRoot, 'classify.md'), 'claude classify', 'utf8');
    writeFileSync(join(defaultRoot, 'classify.md'), 'default classify', 'utf8');
    const promptConfig = parseAdwConfig({
      prompts: {
        defaultRoot,
        runnerRoots: { claude: claudeRoot },
      },
    });

    expect(templatePath('claude', 'classify', promptConfig)).toBe(join(claudeRoot, 'classify.md'));
    expect(templatePath('pi', 'classify', promptConfig)).toBe(join(defaultRoot, 'classify.md'));
  });

  it('keeps the docs/examples/payments-api.config.json valid against the schema (drift guard)', () => {
    const example = JSON.parse(
      readFileSync(join(REPO_ROOT, 'adw_sdlc', 'docs', 'examples', 'payments-api.config.json'), 'utf8'),
    ) as unknown;
    const parsed = parseAdwConfig(example, 'payments-api example');
    expect(parsed.project.id).toBe('payments-api');
    expect(parsed.providers.workItems.inProgressStatus).toBe('In Progress');
    expect(parsed.commands.defaultTestCommand).toBe('npm test');
    expect(parsed.commands.defaultFinalizeGates).toEqual([
      'npm run lint',
      'npm run typecheck',
      'npm run build',
    ]);
    expect(parsed.gates.e2e.hints).toContain('webhook');
    expect(parsed.providers.workItems.doneStatus).toBe('Done');
    // The example pins the full chain explicitly so a future kernel-default
    // change cannot silently reshape this project's pipeline.
    expect(parsed.phases).toEqual([
      'classify',
      'plan',
      'implement',
      'tests',
      'resolve',
      'e2e',
      'review',
      'patch',
      'document',
    ]);
  });

  it('exposes a configurable in-progress status (default "In Progress")', () => {
    expect(parseAdwConfig({}).providers.workItems.inProgressStatus).toBe('In Progress');
    expect(
      parseAdwConfig({ providers: { workItems: { type: 'github', inProgressStatus: 'Doing' } } })
        .providers.workItems.inProgressStatus,
    ).toBe('Doing');
  });

  it('exposes an optional terminal status, unset by default', () => {
    // Absent by default — GitHub auto-closes via "closes #<n>", so no extra
    // status move happens for the committed HealthTech config.
    expect(parseAdwConfig({}).providers.workItems.doneStatus).toBeUndefined();
    expect(
      parseAdwConfig({ providers: { workItems: { type: 'github', doneStatus: 'Done' } } })
        .providers.workItems.doneStatus,
    ).toBe('Done');
    // Blank is rejected at load (min length 1).
    expect(() =>
      parseAdwConfig({ providers: { workItems: { type: 'github', doneStatus: '' } } }),
    ).toThrow(/invalid/);
  });

  it('treats configured closedStates as terminal for the verify gate', () => {
    const defaults = parseAdwConfig({});
    expect(isClosedWorkItemState('CLOSED', defaults)).toBe(true);
    expect(isClosedWorkItemState('OPEN', defaults)).toBe(false);

    const custom = parseAdwConfig({
      providers: { workItems: { type: 'github', closedStates: ['Done', 'Resolved'] } },
    });
    expect(isClosedWorkItemState('Done', custom)).toBe(true);
    expect(isClosedWorkItemState('Resolved', custom)).toBe(true);
    expect(isClosedWorkItemState('CLOSED', custom)).toBe(false);
  });

  it('drives model routing and progress tags', () => {
    expect(classifyModel(config)).toBe('project-classifier');
    expect(modelForPhase('implement', 'claude', { env: {}, config })).toBe('project-sonnet');
    expect(formatProgress('a1b2c3d4', 'plan', 'done', config.progress.tag)).toBe(
      '[ACME-ADW] a1b2c3d4_plan:\n\n### Planning\n\ndone',
    );
  });
});
