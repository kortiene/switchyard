import { describe, expect, it, vi } from 'vitest';

import {
  createGitHubChangeRequestProvider,
  createGitHubWorkItemProvider,
  createProvidersFromConfig,
  providerBackedDeps,
  type AdwProviders,
} from '../src/providers.js';
import { parseAdwConfig } from '../src/config.js';

function fakeProviders(): AdwProviders {
  return {
    cli: {
      resolveExecutable: vi.fn(() => '/bin/gh'),
      detectRepository: vi.fn((ghBin) => (ghBin ? 'owner/repo' : '')),
    },
    workItems: {
      fetch: vi.fn((_ctx, id) => ({ title: `Issue ${id}`, body: 'body', labels: ['bug'] })),
      state: vi.fn((_ctx, id) => (String(id) === '7' ? 'OPEN' : 'UNKNOWN')),
      postProgress: vi.fn(),
      assignSelf: vi.fn(),
      setStatus: vi.fn(),
    },
    vcs: {
      workingTreeDirty: vi.fn(() => false),
      changedFiles: vi.fn(() => ['src/index.ts']),
      createOrCheckoutBranch: vi.fn(() => ({ ok: true, error: null })),
      commitAll: vi.fn(() => ({ ok: true, error: null })),
      push: vi.fn(() => ({ ok: true, error: null })),
      pullRebase: vi.fn(() => ({ ok: true, error: null })),
    },
    changeRequests: {
      findForBranch: vi.fn((_ctx, branch) => `https://example.test/pull/${branch}`),
      create: vi.fn((_ctx, _input) => ({ id: '12', number: 12, url: 'https://example.test/pull/12', error: null })),
      pipelineStatus: vi.fn(() => ({ state: 'success' as const, failingJobs: [] })),
      ciStatus: vi.fn(() => ({ state: 'success' as const, failingJobs: [] })),
      squashMerge: vi.fn(() => ({ ok: true, error: null })),
    },
  };
}

describe('createProvidersFromConfig', () => {
  it('creates the configured built-in Git/GitHub providers', () => {
    const providers = createProvidersFromConfig(parseAdwConfig({}), () => ['src/a.ts']);
    expect(providers.cli.resolveExecutable).toBeTypeOf('function');
    expect(providers.cli.detectRepository).toBeTypeOf('function');
    expect(providers.workItems.fetch).toBeTypeOf('function');
    expect(providers.workItems.assignSelf).toBeTypeOf('function');
    expect(providers.vcs.changedFiles('main')).toEqual(['src/a.ts']);
    expect(providers.changeRequests.create).toBeTypeOf('function');
  });
});

describe('providerBackedDeps', () => {
  it('adapts provider interfaces to the legacy OrchestratorDeps effect seams', () => {
    const providers = fakeProviders();
    const deps = providerBackedDeps(providers);

    expect(deps.resolveGhBin()).toBe('/bin/gh');
    expect(deps.detectRepo('/bin/gh')).toBe('owner/repo');
    expect(deps.issueState('/bin/gh', 7, 'owner/repo')).toBe('OPEN');
    expect(deps.fetchIssue('/bin/gh', 7, 'owner/repo')).toEqual({ title: 'Issue 7', body: 'body', labels: ['bug'] });
    deps.postProgress('/bin/gh', 7, 'owner/repo', 'a1b2c3d4', 'plan', 'done');
    providers.workItems.assignSelf({ ghBin: '/bin/gh', repo: 'owner/repo' }, 7);
    deps.setStatus('/bin/gh', 'owner', 7, 'In Progress');

    expect(deps.workingTreeDirty()).toBe(false);
    expect(deps.changedFiles('main')).toEqual(['src/index.ts']);
    expect(deps.git.createOrCheckoutBranch('feat/7-x', 'main')).toEqual({ ok: true, error: null });
    expect(deps.git.commitAll('msg')).toEqual({ ok: true, error: null });
    expect(deps.git.push('feat/7-x')).toEqual({ ok: true, error: null });
    expect(deps.git.pullRebase('main')).toEqual({ ok: true, error: null });
    expect(deps.git.prForBranch('feat/7-x', '/bin/gh', 'owner/repo')).toBe('https://example.test/pull/feat/7-x');
    expect(providers.changeRequests.create({ ghBin: '/bin/gh', repo: 'owner/repo' }, {
      branch: 'feat/7-x',
      title: 'title',
      body: 'body',
      base: 'main',
    })).toEqual({ id: '12', number: 12, url: 'https://example.test/pull/12', error: null });
    expect(providers.changeRequests.pipelineStatus({ ghBin: '/bin/gh', repo: 'owner/repo' }, 12)).toEqual({
      state: 'success',
      failingJobs: [],
    });
    // Legacy deps adapter keeps the old GitHub PR-shaped return without the provider-neutral id.
    expect(deps.git.createPr('feat/7-x', 'title', 'body', 'main', '/bin/gh', 'owner/repo')).toEqual({
      number: 12,
      url: 'https://example.test/pull/12',
      error: null,
    });
    expect(deps.git.ciStatus(12, '/bin/gh', 'owner/repo')).toEqual({ state: 'success', failingJobs: [] });
    expect(deps.git.squashMerge(12, '/bin/gh', 'owner/repo')).toEqual({ ok: true, error: null });

    expect(providers.workItems.postProgress).toHaveBeenCalledWith(
      { ghBin: '/bin/gh', repo: 'owner/repo' },
      7,
      'a1b2c3d4',
      'plan',
      'done',
    );
    expect(providers.workItems.assignSelf).toHaveBeenCalledWith({ ghBin: '/bin/gh', repo: 'owner/repo' }, 7);
    expect(providers.workItems.setStatus).toHaveBeenCalledWith({ ghBin: '/bin/gh', repo: 'owner' }, 7, 'In Progress');
    expect(providers.changeRequests.create).toHaveBeenCalledWith(
      { ghBin: '/bin/gh', repo: 'owner/repo' },
      { branch: 'feat/7-x', title: 'title', body: 'body', base: 'main' },
    );
  });
});

describe('built-in GitHub provider no-gh fallbacks', () => {
  it('change-request operations fail closed without gh', () => {
    const provider = createGitHubChangeRequestProvider();
    const ctx = { ghBin: null, repo: 'owner/repo' };

    expect(provider.findForBranch(ctx, 'feat/x')).toBeNull();
    expect(provider.create(ctx, { branch: 'feat/x', title: 't', body: 'b', base: 'main' })).toEqual({
      id: null,
      number: null,
      url: null,
      error: 'gh not found',
    });
    expect(provider.pipelineStatus(ctx, 1)).toEqual({ state: 'unknown', failingJobs: [] });
    expect(provider.ciStatus?.(ctx, 1)).toEqual({ state: 'unknown', failingJobs: [] });
    expect(provider.squashMerge(ctx, 1)).toEqual({ ok: false, error: 'gh not found' });
  });

  it('work-item status update is a no-op without gh or owner', () => {
    const provider = createGitHubWorkItemProvider();
    expect(() => provider.assignSelf({ ghBin: null, repo: 'owner/repo' }, 1)).not.toThrow();
    expect(() => provider.setStatus({ ghBin: null, repo: 'owner/repo' }, 1, 'In Progress')).not.toThrow();
    expect(() => provider.setStatus({ ghBin: '/bin/gh', repo: '' }, 1, 'In Progress')).not.toThrow();
  });
});
