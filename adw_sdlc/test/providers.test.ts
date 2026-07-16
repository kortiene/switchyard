import { describe, expect, it, vi } from 'vitest';

import {
  createGitHubChangeRequestProvider,
  createGitHubWorkItemProvider,
  createProvidersFromConfig,
  providerBackedDeps,
  supportedProviderTypes,
  type AdwProviders,
} from '../src/providers.js';
import { parseAdwConfig } from '../src/config.js';
import {
  parseCliChangeRequestDescriptor,
  parseCliWorkItemDescriptor,
  parseRestChangeRequestDescriptor,
  parseRestWorkItemDescriptor,
} from '../src/provider-descriptor.js';
import {
  createCliChangeRequestProvider,
  createCliWorkItemProvider,
  createRestChangeRequestProvider,
  createRestWorkItemProvider,
  type RestRequest,
  type RestTransport,
} from '../src/providers-rest-cli.js';
import { formatProgress, type Captured } from '../src/exec.js';
import { withScopedEnv } from './helpers.js';

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
      syncWithBase: vi.fn(() => ({ ok: true, rebased: false, error: null })),
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

  it('reports the registered built-in provider kinds', () => {
    expect(supportedProviderTypes()).toEqual({
      cli: ['github'],
      workItems: ['cli', 'github', 'rest'],
      vcs: ['git'],
      changeRequests: ['cli', 'github', 'rest'],
    });
  });

  it('fails closed on an unknown provider kind, naming the role and supported types', () => {
    // Config shape-validates the type string; the registry owns membership and
    // throws a loud AdwError before any provider is built (run-start fail-closed).
    const badVcs = parseAdwConfig({ providers: { vcs: { type: 'svn' } } });
    expect(() => createProvidersFromConfig(badVcs, () => [])).toThrow(
      /unsupported vcs provider type "svn" \(supported: git\)/,
    );

    const badWorkItems = parseAdwConfig({ providers: { workItems: { type: 'jira' } } });
    expect(() => createProvidersFromConfig(badWorkItems, () => [])).toThrow(
      /unsupported workItems provider type "jira" \(supported: cli, github, rest\)/,
    );
  });
});

describe('declarative cli work-item provider', () => {
  const descriptorRaw = {
    type: 'cli',
    authEnv: 'GITLAB_TOKEN',
    routes: {
      fetch: {
        command: ['glab', 'issue', 'view', '{id}', '--repo', '{repo}', '--output', 'json'],
        map: { title: '$.title', body: '$.description', labels: '$.labels[*].name' },
      },
      state: { command: ['glab', 'issue', 'view', '{id}', '--output', 'json'], map: { state: '$.state' } },
    },
  };
  const descriptor = parseCliWorkItemDescriptor(descriptorRaw);

  it('substitutes placeholders, maps JSON, and scopes the env to one credential (GH_TOKEN withheld)', () => {
    const calls: { cmd: readonly string[]; env?: Record<string, string> }[] = [];
    const fakeCapture = (cmd: readonly string[], opts?: { env?: Record<string, string> }): Captured => {
      calls.push({ cmd, env: opts?.env });
      return {
        returncode: 0,
        stdout: JSON.stringify({ title: 'Fix login', description: 'b', state: 'opened', labels: [{ name: 'bug' }] }),
        stderr: '',
      };
    };

    withScopedEnv({ GITLAB_TOKEN: 'secret-token', GH_TOKEN: 'gh-secret' }, () => {
      const provider = createCliWorkItemProvider(descriptor, fakeCapture);
      expect(provider.fetch({ ghBin: null, repo: 'group/proj' }, 42)).toEqual({
        title: 'Fix login',
        body: 'b',
        labels: ['bug'],
      });
      const first = calls[0]!;
      expect(first.cmd).toEqual(['glab', 'issue', 'view', '42', '--repo', 'group/proj', '--output', 'json']);
      expect(first.env?.['GITLAB_TOKEN']).toBe('secret-token'); // one credential in
      expect(first.env?.['GH_TOKEN']).toBeUndefined(); // ambient GitHub authority withheld
    });
  });

  it('falls back to UNKNOWN state and null fetch on failure or unparseable output', () => {
    const fail = (): Captured => ({ returncode: 1, stdout: '', stderr: 'boom' });
    const failing = createCliWorkItemProvider(descriptor, fail);
    expect(failing.state({ ghBin: null, repo: '' }, 1)).toBe('UNKNOWN');
    expect(failing.fetch({ ghBin: null, repo: '' }, 1)).toBeNull();

    const garbage = (): Captured => ({ returncode: 0, stdout: 'not json', stderr: '' });
    expect(createCliWorkItemProvider(descriptor, garbage).state({ ghBin: null, repo: '' }, 1)).toBe('UNKNOWN');
  });

  it('no-ops optional write routes that are not configured', () => {
    let called = false;
    const spy = (): Captured => {
      called = true;
      return { returncode: 0, stdout: '', stderr: '' };
    };
    const provider = createCliWorkItemProvider(descriptor, spy);
    provider.postProgress({ ghBin: null, repo: '' }, 1, 'a1b2c3d4', 'plan', 'msg');
    provider.assignSelf({ ghBin: null, repo: '' }, 1);
    provider.setStatus({ ghBin: null, repo: '' }, 1, 'Done');
    expect(called).toBe(false);
  });

  it('fail-closed guard (cli): doneStatus without setStatus route throws at construction', () => {
    const config = parseAdwConfig({ providers: { workItems: { ...descriptorRaw, doneStatus: 'Done' } } });
    expect(() => createProvidersFromConfig(config, () => [])).toThrow(/doneStatus .* no .* setStatus route/);
  });

  it('fail-closed guard (cli): doneStatus with setStatus route does not throw', () => {
    const config = parseAdwConfig({
      providers: {
        workItems: {
          ...descriptorRaw,
          doneStatus: 'Done',
          routes: {
            ...descriptorRaw.routes,
            setStatus: { command: ['glab', 'issue', 'reopen', '{id}'] },
          },
        },
      },
    });
    expect(() => createProvidersFromConfig(config, () => [])).not.toThrow();
  });

  it('is built through createProvidersFromConfig for type: "cli"', () => {
    const config = parseAdwConfig({ providers: { workItems: descriptorRaw } });
    const providers = createProvidersFromConfig(config, () => []);
    expect(providers.workItems.fetch).toBeTypeOf('function');
    expect(providers.workItems.state).toBeTypeOf('function');
  });

  it('fails closed at construction (run start) for a misconfigured cli descriptor', () => {
    // type: "cli" with no routes ⇒ loud AdwError from createProvidersFromConfig,
    // i.e. before defaultDeps returns and before any side effect / dry-run.
    const config = parseAdwConfig({ providers: { workItems: { type: 'cli' } } });
    expect(() => createProvidersFromConfig(config, () => [])).toThrow(/invalid cli work-item provider/);
  });
});

describe('declarative rest work-item provider', () => {
  const raw = {
    type: 'rest',
    baseUrl: 'https://gitlab.example.com/api/v4',
    allowedHosts: ['gitlab.example.com'],
    authEnv: 'GITLAB_TOKEN',
    routes: {
      fetch: {
        path: '/projects/{repo}/issues/{id}',
        map: { title: '$.title', body: '$.description', labels: '$.labels[*].name' },
      },
      state: { path: '/projects/{repo}/issues/{id}', map: { state: '$.state' } },
    },
  };
  const descriptor = parseRestWorkItemDescriptor(raw);

  it('resolves the url with percent-encoded placeholders, maps JSON, and scopes the env', () => {
    const seen: { req: RestRequest; env: Record<string, string> }[] = [];
    const transport: RestTransport = (req, env) => {
      seen.push({ req, env });
      return {
        status: 200,
        body: JSON.stringify({ title: 'T', description: 'B', state: 'opened', labels: [{ name: 'bug' }] }),
      };
    };

    withScopedEnv({ GITLAB_TOKEN: 'tok', GH_TOKEN: 'gh-secret' }, () => {
      const provider = createRestWorkItemProvider(descriptor, transport);
      expect(provider.fetch({ ghBin: null, repo: 'group/proj' }, 42)).toEqual({ title: 'T', body: 'B', labels: ['bug'] });
      const { req, env } = seen[0]!;
      // {repo} is percent-encoded as a single path component; host is unchanged.
      expect(req.url).toBe('https://gitlab.example.com/api/v4/projects/group%2Fproj/issues/42');
      expect(req.authEnv).toBe('GITLAB_TOKEN');
      expect(req.authHeader).toBe('Authorization');
      expect(req.authScheme).toBe('Bearer');
      expect(env['GITLAB_TOKEN']).toBe('tok'); // one credential in
      expect(env['GH_TOKEN']).toBeUndefined(); // GitHub authority withheld
    });
  });

  it('returns null/UNKNOWN on non-2xx, transport error, or unparseable body', () => {
    const p404 = createRestWorkItemProvider(descriptor, () => ({ status: 404, body: '{}' }));
    expect(p404.fetch({ ghBin: null, repo: 'g/p' }, 1)).toBeNull();
    expect(p404.state({ ghBin: null, repo: 'g/p' }, 1)).toBe('UNKNOWN');

    const pErr = createRestWorkItemProvider(descriptor, () => ({ status: 0, body: '', error: 'network' }));
    expect(pErr.fetch({ ghBin: null, repo: 'g/p' }, 1)).toBeNull();

    const pBad = createRestWorkItemProvider(descriptor, () => ({ status: 200, body: 'not json' }));
    expect(pBad.state({ ghBin: null, repo: 'g/p' }, 1)).toBe('UNKNOWN');
  });

  it('unrouted write methods (no route keys) are best-effort no-ops', () => {
    // descriptor has only fetch/state — postProgress/assignSelf/setStatus unrouted
    let called = false;
    const provider = createRestWorkItemProvider(descriptor, () => {
      called = true;
      return { status: 200, body: '{}' };
    });
    provider.postProgress({ ghBin: null, repo: '' }, 1, 'a1', 'plan', 'm');
    provider.assignSelf({ ghBin: null, repo: '' }, 1);
    provider.setStatus({ ghBin: null, repo: '' }, 1, 'Done');
    expect(called).toBe(false);
  });

  it('routed setStatus: issues PUT request with percent-encoded URL, substituted body, and scoped env', () => {
    const rawWithSetStatus = {
      ...raw,
      routes: {
        ...raw.routes,
        setStatus: { method: 'PUT', path: '/projects/{repo}/issues/{id}', body: { state_event: '{status}' } },
      },
    };
    const d = parseRestWorkItemDescriptor(rawWithSetStatus);
    const seen: { req: RestRequest; env: Record<string, string> }[] = [];
    const transport: RestTransport = (req, env) => {
      seen.push({ req, env });
      return { status: 200, body: '{}' };
    };
    withScopedEnv({ GITLAB_TOKEN: 'tok', GH_TOKEN: 'gh-secret' }, () => {
      const provider = createRestWorkItemProvider(d, transport);
      provider.setStatus({ ghBin: null, repo: 'group/proj' }, 42, 'Done');
      expect(seen).toHaveLength(1);
      const { req, env } = seen[0]!;
      expect(req.method).toBe('PUT');
      expect(req.url).toBe('https://gitlab.example.com/api/v4/projects/group%2Fproj/issues/42');
      expect(req.body).toEqual({ state_event: 'Done' });
      expect(env['GITLAB_TOKEN']).toBe('tok');
      expect(env['GH_TOKEN']).toBeUndefined();
    });
  });

  it('routed setStatus: non-ok response is best-effort (does not throw)', () => {
    const rawWithSetStatus = {
      ...raw,
      routes: {
        ...raw.routes,
        setStatus: { method: 'PUT', path: '/projects/{repo}/issues/{id}', body: { state_event: '{status}' } },
      },
    };
    const d = parseRestWorkItemDescriptor(rawWithSetStatus);
    const provider = createRestWorkItemProvider(d, () => ({ status: 422, body: '{"message":"invalid state"}' }));
    // must not throw — write failures are best-effort; orchestrator swallows them
    expect(() => provider.setStatus({ ghBin: null, repo: 'g/p' }, 42, 'Done')).not.toThrow();
  });

  it('routed postProgress: templates the {body} placeholder into the request body', () => {
    const rawWithPostProgress = {
      ...raw,
      routes: {
        ...raw.routes,
        postProgress: { path: '/projects/{repo}/issues/{id}/notes', body: { body: '{body}' } },
      },
    };
    const d = parseRestWorkItemDescriptor(rawWithPostProgress);
    const seen: RestRequest[] = [];
    const transport: RestTransport = (req) => {
      seen.push(req);
      return { status: 201, body: '{}' };
    };
    const provider = createRestWorkItemProvider(d, transport);
    provider.postProgress({ ghBin: null, repo: 'g/p' }, 7, 'a1b2c3d4', 'plan', 'msg');
    expect(seen).toHaveLength(1);
    expect(seen[0]!.body).toEqual({ body: formatProgress('a1b2c3d4', 'plan', 'msg') });
  });

  it('routed postProgress: non-ok response is best-effort (does not throw)', () => {
    const rawWithPostProgress = {
      ...raw,
      routes: {
        ...raw.routes,
        postProgress: { path: '/projects/{repo}/issues/{id}/notes', body: { body: '{body}' } },
      },
    };
    const d = parseRestWorkItemDescriptor(rawWithPostProgress);
    const provider = createRestWorkItemProvider(d, () => ({ status: 503, body: '' }));
    expect(() => provider.postProgress({ ghBin: null, repo: 'g/p' }, 7, 'a1b2c3d4', 'plan', 'msg')).not.toThrow();
  });

  it('routed assignSelf: issues its request with correct method and URL', () => {
    const rawWithAssign = {
      ...raw,
      routes: {
        ...raw.routes,
        assignSelf: { method: 'POST', path: '/projects/{repo}/issues/{id}/assignees' },
      },
    };
    const d = parseRestWorkItemDescriptor(rawWithAssign);
    const seen: RestRequest[] = [];
    const transport: RestTransport = (req) => {
      seen.push(req);
      return { status: 200, body: '{}' };
    };
    const provider = createRestWorkItemProvider(d, transport);
    provider.assignSelf({ ghBin: null, repo: 'g/p' }, 5);
    expect(seen).toHaveLength(1);
    expect(seen[0]!.method).toBe('POST');
    expect(seen[0]!.url).toBe('https://gitlab.example.com/api/v4/projects/g%2Fp/issues/5/assignees');
  });

  it('fail-closed guard (rest): doneStatus without setStatus route throws at construction', () => {
    const config = parseAdwConfig({ providers: { workItems: { ...raw, doneStatus: 'Done' } } });
    expect(() => createProvidersFromConfig(config, () => [])).toThrow(/doneStatus .* no .* setStatus route/);
  });

  it('fail-closed guard (rest): doneStatus with setStatus route builds successfully', () => {
    const config = parseAdwConfig({
      providers: {
        workItems: {
          ...raw,
          doneStatus: 'Done',
          routes: {
            ...raw.routes,
            setStatus: { path: '/projects/{repo}/issues/{id}', body: { state_event: '{status}' } },
          },
        },
      },
    });
    expect(() => createProvidersFromConfig(config, () => [])).not.toThrow();
  });

  it('parseRestWorkItemDescriptor: unknown placeholder in setStatus body throws', () => {
    expect(() =>
      parseRestWorkItemDescriptor({
        ...raw,
        routes: {
          ...raw.routes,
          setStatus: { path: '/projects/{repo}/issues/{id}', body: { evil: '{bogus}' } },
        },
      }),
    ).toThrow(/unknown placeholder/);
  });

  it('parseRestWorkItemDescriptor: disallowed placeholder in write-route path throws', () => {
    expect(() =>
      parseRestWorkItemDescriptor({
        ...raw,
        routes: {
          ...raw.routes,
          setStatus: { path: '/projects/{repo}/issues/{id}/{bogus}' },
        },
      }),
    ).toThrow(/unknown placeholder/);
  });

  it('parseRestWorkItemDescriptor: unknown placeholder in postProgress body throws', () => {
    expect(() =>
      parseRestWorkItemDescriptor({
        ...raw,
        routes: {
          ...raw.routes,
          postProgress: { path: '/projects/{repo}/issues/{id}/notes', body: { note: '{bogus}' } },
        },
      }),
    ).toThrow(/unknown placeholder/);
  });

  it('parseRestWorkItemDescriptor: unknown placeholder in assignSelf path throws', () => {
    expect(() =>
      parseRestWorkItemDescriptor({
        ...raw,
        routes: {
          ...raw.routes,
          assignSelf: { path: '/projects/{repo}/issues/{id}/{bogus}/assignees' },
        },
      }),
    ).toThrow(/unknown placeholder/);
  });

  it('is built through createProvidersFromConfig, and fails closed for an off-allowlist host', () => {
    const good = parseAdwConfig({ providers: { workItems: raw } });
    expect(createProvidersFromConfig(good, () => []).workItems.fetch).toBeTypeOf('function');

    const bad = parseAdwConfig({ providers: { workItems: { ...raw, allowedHosts: ['other.example.com'] } } });
    expect(() => createProvidersFromConfig(bad, () => [])).toThrow(/not in allowedHosts/);
  });
});

describe('declarative rest change-request provider', () => {
  const raw = {
    type: 'rest',
    baseUrl: 'https://gitlab.example.com/api/v4',
    allowedHosts: ['gitlab.example.com'],
    authEnv: 'GITLAB_TOKEN',
    routes: {
      findForBranch: { path: '/projects/{repo}/merge_requests?source_branch={branch}', map: { url: '$[0].web_url' } },
      create: {
        method: 'POST',
        path: '/projects/{repo}/merge_requests',
        body: { source_branch: '{branch}', target_branch: '{base}', title: '{title}', description: '{body}' },
        map: { number: '$.iid', url: '$.web_url' },
      },
      squashMerge: { method: 'PUT', path: '/projects/{repo}/merge_requests/{id}/merge', body: { squash: true } },
      pipelineStatus: {
        path: '/projects/{repo}/merge_requests/{id}',
        statusPath: '$.pipeline.status',
        stateMap: { success: 'success', failed: 'failure', running: 'pending' },
      },
    },
  };
  const descriptor = parseRestChangeRequestDescriptor(raw);

  it('create: substitutes the JSON body, maps number/url/id, and scopes the env', () => {
    const seen: { req: RestRequest; env: Record<string, string> }[] = [];
    const transport: RestTransport = (req, env) => {
      seen.push({ req, env });
      return { status: 201, body: JSON.stringify({ iid: 7, web_url: 'https://gitlab.example.com/g/p/-/merge_requests/7' }) };
    };
    withScopedEnv({ GITLAB_TOKEN: 'tok', GH_TOKEN: 'gh-secret' }, () => {
      const provider = createRestChangeRequestProvider(descriptor, transport);
      const result = provider.create({ ghBin: null, repo: 'group/proj' }, {
        branch: 'feat/7-x',
        base: 'main',
        title: 'My MR',
        body: 'desc',
      });
      expect(result).toEqual({
        id: '7',
        number: 7,
        url: 'https://gitlab.example.com/g/p/-/merge_requests/7',
        error: null,
      });
      const { req, env } = seen[0]!;
      expect(req.method).toBe('POST');
      expect(req.url).toBe('https://gitlab.example.com/api/v4/projects/group%2Fproj/merge_requests');
      expect(req.body).toEqual({ source_branch: 'feat/7-x', target_branch: 'main', title: 'My MR', description: 'desc' });
      expect(env['GITLAB_TOKEN']).toBe('tok');
      expect(env['GH_TOKEN']).toBeUndefined();
    });
  });

  it('findForBranch maps a url from the response, or null when none', () => {
    const found = createRestChangeRequestProvider(descriptor, () => ({
      status: 200,
      body: JSON.stringify([{ web_url: 'https://gitlab.example.com/x/1' }]),
    }));
    expect(found.findForBranch({ ghBin: null, repo: 'g/p' }, 'feat/x')).toBe('https://gitlab.example.com/x/1');
    const none = createRestChangeRequestProvider(descriptor, () => ({ status: 200, body: '[]' }));
    expect(none.findForBranch({ ghBin: null, repo: 'g/p' }, 'feat/x')).toBeNull();
  });

  it('squashMerge issues the templated PUT and reports ok/failure', () => {
    const seen: RestRequest[] = [];
    const merged = createRestChangeRequestProvider(descriptor, (req) => {
      seen.push(req);
      return { status: 200, body: '{}' };
    });
    expect(merged.squashMerge({ ghBin: null, repo: 'g/p' }, 7)).toEqual({ ok: true, error: null });
    expect(seen[0]!.method).toBe('PUT');
    expect(seen[0]!.url).toBe('https://gitlab.example.com/api/v4/projects/g%2Fp/merge_requests/7/merge');
    expect(seen[0]!.body).toEqual({ squash: true });

    const failed = createRestChangeRequestProvider(descriptor, () => ({ status: 405, body: 'no' }));
    expect(failed.squashMerge({ ghBin: null, repo: 'g/p' }, 7)).toEqual({ ok: false, error: 'merge failed (status 405)' });
  });

  it('pipelineStatus maps the forge status via stateMap; an absent route ⇒ none', () => {
    const green = createRestChangeRequestProvider(descriptor, () => ({ status: 200, body: JSON.stringify({ pipeline: { status: 'success' } }) }));
    expect(green.pipelineStatus({ ghBin: null, repo: 'g/p' }, 7)).toEqual({ state: 'success', failingJobs: [] });
    const red = createRestChangeRequestProvider(descriptor, () => ({ status: 200, body: JSON.stringify({ pipeline: { status: 'failed' } }) }));
    expect(red.pipelineStatus({ ghBin: null, repo: 'g/p' }, 7).state).toBe('failure');
    const weird = createRestChangeRequestProvider(descriptor, () => ({ status: 200, body: JSON.stringify({ pipeline: { status: 'wat' } }) }));
    expect(weird.pipelineStatus({ ghBin: null, repo: 'g/p' }, 7).state).toBe('unknown'); // unmapped ⇒ unknown

    const noPipe = parseRestChangeRequestDescriptor({
      ...raw,
      routes: { findForBranch: raw.routes.findForBranch, create: raw.routes.create, squashMerge: raw.routes.squashMerge },
    });
    const provider = createRestChangeRequestProvider(noPipe, () => ({ status: 500, body: '' }));
    expect(provider.pipelineStatus({ ghBin: null, repo: 'g/p' }, 7)).toEqual({ state: 'none', failingJobs: [] });
  });

  it('is built through createProvidersFromConfig, and fails closed for an off-allowlist host', () => {
    const good = parseAdwConfig({ providers: { changeRequests: raw } });
    expect(createProvidersFromConfig(good, () => []).changeRequests.squashMerge).toBeTypeOf('function');

    const bad = parseAdwConfig({ providers: { changeRequests: { ...raw, allowedHosts: ['other.example.com'] } } });
    expect(() => createProvidersFromConfig(bad, () => [])).toThrow(/not in allowedHosts/);
  });
});

describe('declarative cli change-request provider', () => {
  const descriptorRaw = {
    type: 'cli',
    authEnv: 'GITLAB_TOKEN',
    routes: {
      findForBranch: {
        command: ['glab', 'mr', 'list', '--repo', '{repo}', '--source-branch', '{branch}', '--output', 'json'],
        map: { url: '$[0].web_url' },
      },
      create: {
        command: ['glab', 'mr', 'create', '--repo', '{repo}', '--source-branch', '{branch}', '--target-branch', '{base}', '--title', '{title}', '--description', '{body}', '--output', 'json'],
        map: { number: '$.iid', url: '$.web_url' },
      },
      squashMerge: { command: ['glab', 'mr', 'merge', '{id}', '--repo', '{repo}', '--squash', '--yes'] },
      pipelineStatus: {
        command: ['glab', 'ci', 'status', '--repo', '{repo}', '{id}', '--output', 'json'],
        statusPath: '$.status | lower',
        stateMap: { success: 'success', failed: 'failure' },
      },
      failingJobs: {
        command: ['glab', 'ci', 'list', '--repo', '{repo}', '{id}', '--status', 'failed', '--output', 'json'],
        itemsPath: '$.jobs',
        map: [{ name: '$.name', logExcerpt: '$.failure_reason | default:(none)' }],
      },
    },
  };
  const descriptor = parseCliChangeRequestDescriptor(descriptorRaw);
  const ctx = { ghBin: null, repo: 'group/proj' };

  it('create: substitutes argv, scopes the env to one credential (GH_TOKEN withheld), maps number/url/id', () => {
    const calls: { cmd: readonly string[]; env?: Record<string, string> }[] = [];
    const fakeCapture = (cmd: readonly string[], opts?: { env?: Record<string, string> }): Captured => {
      calls.push({ cmd, env: opts?.env });
      return { returncode: 0, stdout: JSON.stringify({ iid: 7, web_url: 'https://gitlab.example.com/g/p/-/merge_requests/7' }), stderr: '' };
    };
    withScopedEnv({ GITLAB_TOKEN: 'tok', GH_TOKEN: 'gh-secret' }, () => {
      const result = createCliChangeRequestProvider(descriptor, fakeCapture).create(ctx, {
        branch: 'feat/7-x',
        base: 'main',
        title: 'My MR',
        body: 'desc',
      });
      expect(result).toEqual({ id: '7', number: 7, url: 'https://gitlab.example.com/g/p/-/merge_requests/7', error: null });
      const first = calls[0]!;
      expect(first.cmd).toEqual(['glab', 'mr', 'create', '--repo', 'group/proj', '--source-branch', 'feat/7-x', '--target-branch', 'main', '--title', 'My MR', '--description', 'desc', '--output', 'json']);
      expect(first.env?.['GITLAB_TOKEN']).toBe('tok'); // one credential in
      expect(first.env?.['GH_TOKEN']).toBeUndefined(); // GitHub authority withheld
    });
  });

  it('findForBranch maps a url from the response, null when none, null on failure', () => {
    const found = createCliChangeRequestProvider(descriptor, () => ({ returncode: 0, stdout: JSON.stringify([{ web_url: 'https://x/1' }]), stderr: '' }));
    expect(found.findForBranch(ctx, 'feat/x')).toBe('https://x/1');
    const none = createCliChangeRequestProvider(descriptor, () => ({ returncode: 0, stdout: '[]', stderr: '' }));
    expect(none.findForBranch(ctx, 'feat/x')).toBeNull();
    const fail = createCliChangeRequestProvider(descriptor, () => ({ returncode: 1, stdout: '', stderr: 'boom' }));
    expect(fail.findForBranch(ctx, 'feat/x')).toBeNull();
  });

  it('squashMerge runs the templated command and reports ok/failure (stderr surfaced)', () => {
    const seen: (readonly string[])[] = [];
    const merged = createCliChangeRequestProvider(descriptor, (cmd) => {
      seen.push(cmd);
      return { returncode: 0, stdout: '', stderr: '' };
    });
    expect(merged.squashMerge(ctx, 7)).toEqual({ ok: true, error: null });
    expect(seen[0]).toEqual(['glab', 'mr', 'merge', '7', '--repo', 'group/proj', '--squash', '--yes']);
    const failed = createCliChangeRequestProvider(descriptor, () => ({ returncode: 1, stdout: '', stderr: 'merge conflict\n' }));
    expect(failed.squashMerge(ctx, 7)).toEqual({ ok: false, error: 'merge conflict' });
  });

  it('pipelineStatus maps via stateMap (after a `| lower` transform) and fills failingJobs when red', () => {
    const transport = (cmd: readonly string[]): Captured => {
      if (cmd[1] === 'ci' && cmd[2] === 'status') return { returncode: 0, stdout: JSON.stringify({ status: 'FAILED' }), stderr: '' };
      if (cmd[1] === 'ci' && cmd[2] === 'list') {
        return { returncode: 0, stdout: JSON.stringify({ jobs: [{ name: 'build', failure_reason: 'tsc' }, { name: 'lint', failure_reason: '' }] }), stderr: '' };
      }
      return { returncode: 1, stdout: '', stderr: '' };
    };
    const status = createCliChangeRequestProvider(descriptor, transport).pipelineStatus(ctx, 7);
    expect(status.state).toBe('failure'); // 'FAILED' | lower ⇒ 'failed' ⇒ failure
    expect(status.failingJobs).toEqual([
      { name: 'build', logExcerpt: 'tsc' },
      { name: 'lint', logExcerpt: '(none)' }, // default applied to the empty reason
    ]);
  });

  it('pipelineStatus is green without enumerating jobs (no extra invocation)', () => {
    let listCalled = false;
    const transport = (cmd: readonly string[]): Captured => {
      if (cmd[1] === 'ci' && cmd[2] === 'status') return { returncode: 0, stdout: JSON.stringify({ status: 'success' }), stderr: '' };
      if (cmd[1] === 'ci' && cmd[2] === 'list') {
        listCalled = true;
        return { returncode: 0, stdout: '{"jobs":[]}', stderr: '' };
      }
      return { returncode: 1, stdout: '', stderr: '' };
    };
    expect(createCliChangeRequestProvider(descriptor, transport).pipelineStatus(ctx, 7)).toEqual({ state: 'success', failingJobs: [] });
    expect(listCalled).toBe(false); // jobs only enumerated when red
  });

  it('is built through createProvidersFromConfig for type "cli", and fails closed when misconfigured', () => {
    const good = parseAdwConfig({ providers: { changeRequests: descriptorRaw } });
    expect(createProvidersFromConfig(good, () => []).changeRequests.squashMerge).toBeTypeOf('function');

    const bad = parseAdwConfig({ providers: { changeRequests: { type: 'cli' } } });
    expect(() => createProvidersFromConfig(bad, () => [])).toThrow(/invalid cli change-request provider/);
  });
});

describe('declarative rest provider — transforms + pagination (step 2.5)', () => {
  const base = {
    type: 'rest',
    baseUrl: 'https://gitlab.example.com/api/v4',
    allowedHosts: ['gitlab.example.com'],
    authEnv: 'GITLAB_TOKEN',
    routes: {
      findForBranch: { path: '/projects/{repo}/merge_requests?source_branch={branch}', map: { url: '$[0].web_url' } },
      create: {
        method: 'POST',
        path: '/projects/{repo}/merge_requests',
        body: { source_branch: '{branch}' },
        map: { number: '$.iid', url: '$.web_url' },
      },
      squashMerge: { method: 'PUT', path: '/projects/{repo}/merge_requests/{id}/merge' },
    },
  };
  const ctx = { ghBin: null, repo: 'g/p' };
  const provider = (raw: unknown, transport: RestTransport) =>
    createRestChangeRequestProvider(parseRestChangeRequestDescriptor(raw), transport);

  it('2.5a: a transform normalizes the forge status before the stateMap lookup', () => {
    const raw = {
      ...base,
      routes: {
        ...base.routes,
        // Forge reports SCREAMING status; `| lower` makes the lowercase stateMap match.
        pipelineStatus: {
          path: '/projects/{repo}/merge_requests/{id}',
          statusPath: '$.pipeline.status | lower',
          stateMap: { success: 'success', failed: 'failure' },
        },
      },
    };
    const p = provider(raw, () => ({ status: 200, body: JSON.stringify({ pipeline: { status: 'FAILED' } }) }));
    // failure + no failingJobs route ⇒ empty job list (the route is what populates it).
    expect(p.pipelineStatus(ctx, 7)).toEqual({ state: 'failure', failingJobs: [] });
  });

  it('2.5b: nextUrl pagination accumulates failingJobs across pages, applying item transforms', () => {
    const page2 = 'https://gitlab.example.com/api/v4/projects/g%2Fp/merge_requests/7/jobs?scope=failed&page=2';
    const raw = {
      ...base,
      routes: {
        ...base.routes,
        pipelineStatus: { path: '/projects/{repo}/merge_requests/{id}', statusPath: '$.pipeline.status', stateMap: { failed: 'failure' } },
        failingJobs: {
          path: '/projects/{repo}/merge_requests/{id}/jobs?scope=failed',
          itemsPath: '$.jobs',
          map: [{ name: '$.name', logExcerpt: '$.failure_reason | default:(none)' }],
          paginate: { next: { style: 'nextUrl', path: '$.next' } },
        },
      },
    };
    const seen: string[] = [];
    const transport: RestTransport = (req) => {
      seen.push(req.url);
      if (req.url.includes('/jobs')) {
        if (req.url.includes('page=2')) {
          return { status: 200, body: JSON.stringify({ jobs: [{ name: 'test', failure_reason: '' }], next: '' }) };
        }
        return { status: 200, body: JSON.stringify({ jobs: [{ name: 'build', failure_reason: 'compile error' }], next: page2 }) };
      }
      return { status: 200, body: JSON.stringify({ pipeline: { status: 'failed' } }) };
    };
    const status = provider(raw, transport).pipelineStatus(ctx, 7);
    expect(status.state).toBe('failure');
    expect(status.failingJobs).toEqual([
      { name: 'build', logExcerpt: 'compile error' },
      { name: 'test', logExcerpt: '(none)' }, // default applied to the empty reason
    ]);
    expect(seen).toContain(page2); // the second page was actually followed
  });

  it('2.5b: pageParam pagination accumulates until a page yields zero items', () => {
    const raw = {
      ...base,
      routes: {
        ...base.routes,
        pipelineStatus: { path: '/projects/{repo}/merge_requests/{id}', statusPath: '$.pipeline.status', stateMap: { failed: 'failure' } },
        failingJobs: {
          path: '/projects/{repo}/merge_requests/{id}/jobs',
          itemsPath: '$',
          map: [{ name: '$.name', logExcerpt: '$.reason' }],
          paginate: { next: { style: 'pageParam', param: 'page', start: 1 } },
        },
      },
    };
    const transport: RestTransport = (req) => {
      if (req.url.includes('/jobs')) {
        if (req.url.includes('page=1')) return { status: 200, body: JSON.stringify([{ name: 'a', reason: 'r1' }]) };
        if (req.url.includes('page=2')) return { status: 200, body: JSON.stringify([{ name: 'b', reason: 'r2' }]) };
        return { status: 200, body: '[]' }; // page 3 ⇒ empty ⇒ stop
      }
      return { status: 200, body: JSON.stringify({ pipeline: { status: 'failed' } }) };
    };
    expect(provider(raw, transport).pipelineStatus(ctx, 7).failingJobs).toEqual([
      { name: 'a', logExcerpt: 'r1' },
      { name: 'b', logExcerpt: 'r2' },
    ]);
  });

  it('2.5b: maxPages is a hard cap and the truncation is logged (no silent cap)', () => {
    const raw = {
      ...base,
      routes: {
        ...base.routes,
        pipelineStatus: { path: '/projects/{repo}/merge_requests/{id}', statusPath: '$.pipeline.status', stateMap: { failed: 'failure' } },
        failingJobs: {
          path: '/projects/{repo}/merge_requests/{id}/jobs',
          itemsPath: '$.jobs',
          map: [{ name: '$.name', logExcerpt: '$.reason' }],
          // next always points at a fresh allowlisted page ⇒ would loop forever without the cap.
          paginate: { next: { style: 'nextUrl', path: '$.next' }, maxPages: 2 },
        },
      },
    };
    const nextUrl = 'https://gitlab.example.com/api/v4/projects/g%2Fp/merge_requests/7/jobs';
    const transport: RestTransport = (req) =>
      req.url.includes('/jobs')
        ? { status: 200, body: JSON.stringify({ jobs: [{ name: 'j', reason: 'r' }], next: nextUrl }) }
        : { status: 200, body: JSON.stringify({ pipeline: { status: 'failed' } }) };
    const spy = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    try {
      const jobs = provider(raw, transport).pipelineStatus(ctx, 7).failingJobs;
      expect(jobs).toHaveLength(2); // capped at maxPages, not infinite
    } finally {
      const logged = spy.mock.calls.map((c) => String(c[0])).join('');
      spy.mockRestore();
      expect(logged).toMatch(/pagination truncated at maxPages=2/);
    }
  });

  it('2.5b security: an off-allowlist next URL stops pagination instead of being followed', () => {
    const raw = {
      ...base,
      routes: {
        ...base.routes,
        pipelineStatus: { path: '/projects/{repo}/merge_requests/{id}', statusPath: '$.pipeline.status', stateMap: { failed: 'failure' } },
        failingJobs: {
          path: '/projects/{repo}/merge_requests/{id}/jobs',
          itemsPath: '$.jobs',
          map: [{ name: '$.name', logExcerpt: '$.reason' }],
          paginate: { next: { style: 'nextUrl', path: '$.next' } },
        },
      },
    };
    const seen: string[] = [];
    const transport: RestTransport = (req) => {
      seen.push(req.url);
      if (req.url.includes('/jobs')) {
        // page 1 points the next cursor at an OFF-allowlist host (data exfil attempt).
        return { status: 200, body: JSON.stringify({ jobs: [{ name: 'a', reason: 'r' }], next: 'https://evil.example.com/api/v4/jobs' }) };
      }
      return { status: 200, body: JSON.stringify({ pipeline: { status: 'failed' } }) };
    };
    const spy = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    try {
      const jobs = provider(raw, transport).pipelineStatus(ctx, 7).failingJobs;
      expect(jobs).toEqual([{ name: 'a', logExcerpt: 'r' }]); // only the first page was kept
    } finally {
      const logged = spy.mock.calls.map((c) => String(c[0])).join('');
      spy.mockRestore();
      expect(logged).toMatch(/pagination stopped: next-page host/);
    }
    expect(seen.some((u) => u.includes('evil.example.com'))).toBe(false); // never fetched the evil host
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

  it('validates string ids locally instead of retargeting a numeric prefix', () => {
    const provider = createGitHubWorkItemProvider();
    const ctx = { ghBin: null, repo: 'owner/repo' };

    expect(provider.state(ctx, '12')).toBe('UNKNOWN');
    for (const id of ['PROJ-123', '12junk', '0', String(Number.MAX_SAFE_INTEGER + 1)]) {
      expect(() => provider.state(ctx, id)).toThrow(/GitHub work-item id must be a positive/);
      expect(() => provider.assignSelf(ctx, id)).toThrow(/GitHub work-item id must be a positive/);
    }
  });
});
