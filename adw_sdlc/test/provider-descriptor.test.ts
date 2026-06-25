import { describe, expect, it } from 'vitest';

import {
  assertAllowedHost,
  evalArray,
  evalItems,
  evalScalar,
  evalScalarMapping,
  isAllowedHost,
  parseCliChangeRequestDescriptor,
  parseCliWorkItemDescriptor,
  parsePath,
  parseRestChangeRequestDescriptor,
  parseRestWorkItemDescriptor,
  type CliWorkItemDescriptor,
} from '../src/provider-descriptor.js';

describe('parsePath', () => {
  it('parses object, index, and wildcard segments', () => {
    expect(parsePath('$.title')).toEqual([{ kind: 'key', key: 'title' }]);
    expect(parsePath('$.a.b_c')).toEqual([
      { kind: 'key', key: 'a' },
      { kind: 'key', key: 'b_c' },
    ]);
    expect(parsePath('$.labels[0]')).toEqual([
      { kind: 'key', key: 'labels' },
      { kind: 'index', index: 0 },
    ]);
    expect(parsePath('$.labels[*].name')).toEqual([
      { kind: 'key', key: 'labels' },
      { kind: 'wildcard' },
      { kind: 'key', key: 'name' },
    ]);
  });

  it('rejects malformed paths and more than one wildcard', () => {
    expect(() => parsePath('title')).toThrow(/must start with \$/);
    expect(() => parsePath('$.a.')).toThrow(/invalid map path/);
    expect(() => parsePath('$.a[1')).toThrow(/invalid map path/);
    expect(() => parsePath('$.a-b')).toThrow(/invalid map path/);
    expect(() => parsePath('$.a[*].b[*]')).toThrow(/at most one \[\*\]/);
  });
});

describe('evalScalar / evalArray', () => {
  const data = {
    title: 'Fix login',
    description: 'body text',
    iid: 42,
    state: 'opened',
    labels: [{ name: 'bug' }, { name: 'urgent' }],
    tags: ['a', 'b'],
  };

  it('resolves scalars with gh-style coercion (missing ⇒ "")', () => {
    expect(evalScalar(data, parsePath('$.title'))).toBe('Fix login');
    expect(evalScalar(data, parsePath('$.iid'))).toBe('42'); // number coerced
    expect(evalScalar(data, parsePath('$.missing'))).toBe('');
    expect(evalScalar(data, parsePath('$.labels[0].name'))).toBe('bug');
  });

  it('resolves arrays of strings and objects, missing/non-array ⇒ []', () => {
    expect(evalArray(data, parsePath('$.tags[*]'))).toEqual(['a', 'b']);
    expect(evalArray(data, parsePath('$.labels[*].name'))).toEqual(['bug', 'urgent']);
    expect(evalArray(data, parsePath('$.title[*]'))).toEqual([]); // not an array
    expect(evalArray(data, parsePath('$.missing[*]'))).toEqual([]);
  });

  it('evalItems resolves a wildcard-free path to a raw array (missing/non-array ⇒ [])', () => {
    expect(evalItems({ jobs: [{ name: 'a' }, { name: 'b' }] }, parsePath('$.jobs'))).toEqual([
      { name: 'a' },
      { name: 'b' },
    ]);
    expect(evalItems([1, 2, 3], parsePath('$'))).toEqual([1, 2, 3]); // $ ⇒ the body itself
    expect(evalItems({ jobs: 'nope' }, parsePath('$.jobs'))).toEqual([]); // not an array
    expect(evalItems({}, parsePath('$.missing'))).toEqual([]);
  });
});

describe('evalScalarMapping (step 2.5a transforms)', () => {
  // A transform chain is parsed at compile time; here we drive it through the
  // descriptor compile + evalScalarMapping (the same path the drivers use).
  const compileState = (expr: string) =>
    parseCliWorkItemDescriptor({
      type: 'cli',
      routes: {
        fetch: { command: ['x', '{id}'], map: { title: '$.t', body: '$.b', labels: '$.l[*]' } },
        state: { command: ['x', '{id}'], map: { state: expr } },
      },
    }).routes.state.state;

  it('applies lower/upper/trim and default (present vs missing)', () => {
    expect(evalScalarMapping({ s: 'OPENED' }, compileState('$.s | lower'))).toBe('opened');
    expect(evalScalarMapping({ s: 'opened' }, compileState('$.s | upper'))).toBe('OPENED');
    expect(evalScalarMapping({ s: '  hi  ' }, compileState('$.s | trim'))).toBe('hi');
    expect(evalScalarMapping({}, compileState('$.missing | default:0'))).toBe('0'); // missing ⇒ literal
    expect(evalScalarMapping({ s: 'x' }, compileState('$.s | default:0'))).toBe('x'); // present ⇒ kept
  });

  it('chains transforms left-to-right (normalize then default)', () => {
    expect(evalScalarMapping({ s: '  MERGED ' }, compileState('$.s | trim | lower'))).toBe('merged');
    expect(evalScalarMapping({}, compileState('$.missing | lower | default:none'))).toBe('none');
  });

  it('rejects an unknown transform or a `default` without an argument at compile time', () => {
    expect(() => compileState('$.s | bogus')).toThrow(/unknown transform "bogus"/);
    expect(() => compileState('$.s | default')).toThrow(/transform "default" requires an argument/);
    // `default:` with an empty arg is allowed (substitutes "").
    expect(evalScalarMapping({}, compileState('$.missing | default:'))).toBe('');
  });
});

describe('parseCliWorkItemDescriptor', () => {
  const valid = {
    type: 'cli',
    authEnv: 'GITLAB_TOKEN',
    routes: {
      fetch: {
        command: ['glab', 'issue', 'view', '{id}', '--output', 'json'],
        map: { title: '$.title', body: '$.description', labels: '$.labels[*].name' },
      },
      state: {
        command: ['glab', 'issue', 'view', '{id}', '--output', 'json'],
        map: { state: '$.state' },
      },
    },
  };

  it('compiles a valid descriptor (paths pre-parsed)', () => {
    const d: CliWorkItemDescriptor = parseCliWorkItemDescriptor(valid);
    expect(d.authEnv).toBe('GITLAB_TOKEN');
    expect(d.routes.fetch.command).toEqual(['glab', 'issue', 'view', '{id}', '--output', 'json']);
    expect(d.routes.fetch.title).toEqual({ segments: [{ kind: 'key', key: 'title' }], transforms: [] });
    expect(d.routes.fetch.labels).toEqual([
      { kind: 'key', key: 'labels' },
      { kind: 'wildcard' },
      { kind: 'key', key: 'name' },
    ]);
    expect(d.routes.state.state).toEqual({ segments: [{ kind: 'key', key: 'state' }], transforms: [] });
  });

  it('keeps optional write routes when present and absent', () => {
    expect(parseCliWorkItemDescriptor(valid).routes.postProgress).toBeUndefined();
    const withWrites = parseCliWorkItemDescriptor({
      ...valid,
      routes: {
        ...valid.routes,
        postProgress: { command: ['glab', 'issue', 'note', '{id}', '--message', '{body}'] },
        setStatus: { command: ['glab', 'issue', 'update', '{id}', '--label', '{status}'] },
      },
    });
    expect(withWrites.routes.postProgress?.command).toContain('{body}');
    expect(withWrites.routes.setStatus?.command).toContain('{status}');
  });

  it('rejects missing required routes and bad route shape', () => {
    expect(() => parseCliWorkItemDescriptor({ type: 'cli', routes: { state: valid.routes.state } })).toThrow(
      /invalid cli work-item provider.*fetch/,
    );
    // Unknown field inside a route (strict) is a loud error, not a silent drop.
    expect(() =>
      parseCliWorkItemDescriptor({
        type: 'cli',
        routes: { fetch: { ...valid.routes.fetch, extra: 1 }, state: valid.routes.state },
      }),
    ).toThrow(/invalid cli work-item provider/);
  });

  it('rejects a scalar field given an array path and vice versa', () => {
    expect(() =>
      parseCliWorkItemDescriptor({
        ...valid,
        routes: { ...valid.routes, fetch: { ...valid.routes.fetch, map: { title: '$.title[*]', body: '$.b', labels: '$.l[*]' } } },
      }),
    ).toThrow(/fetch\.map\.title must be a scalar path/);
    expect(() =>
      parseCliWorkItemDescriptor({
        ...valid,
        routes: { ...valid.routes, fetch: { ...valid.routes.fetch, map: { title: '$.t', body: '$.b', labels: '$.labels' } } },
      }),
    ).toThrow(/fetch\.map\.labels must be an array path/);
  });

  it('rejects an unknown command placeholder', () => {
    expect(() =>
      parseCliWorkItemDescriptor({
        ...valid,
        routes: { ...valid.routes, fetch: { ...valid.routes.fetch, command: ['glab', 'view', '{bogus}'] } },
      }),
    ).toThrow(/unknown placeholder \{bogus\}/);
  });

  it('rejects an authEnv that is GH_TOKEN, deny-prefixed, or a model credential', () => {
    for (const authEnv of ['GH_TOKEN', 'GH_BIN', 'ADW_ASSUME_YES', 'MX_AGENT_YES', 'MATRIX_TOKEN', 'ANTHROPIC_API_KEY', 'OPENAI_API_KEY']) {
      expect(() => parseCliWorkItemDescriptor({ ...valid, authEnv })).toThrow(
        /reserved \(GitHub or model credential\)|denied secret prefix/,
      );
    }
  });
});

describe('parseRestWorkItemDescriptor', () => {
  const valid = {
    type: 'rest',
    baseUrl: 'https://gitlab.example.com/api/v4',
    allowedHosts: ['gitlab.example.com'],
    authEnv: 'GITLAB_TOKEN',
    routes: {
      fetch: {
        method: 'GET',
        path: '/projects/{repo}/issues/{id}',
        map: { title: '$.title', body: '$.description', labels: '$.labels[*].name' },
      },
      state: { path: '/projects/{repo}/issues/{id}', map: { state: '$.state' } }, // method defaults to GET
    },
  };

  it('compiles a valid descriptor with method/auth defaults', () => {
    const d = parseRestWorkItemDescriptor(valid);
    expect(d.baseUrl).toBe('https://gitlab.example.com/api/v4');
    expect(d.authHeader).toBe('Authorization');
    expect(d.authScheme).toBe('Bearer');
    expect(d.routes.fetch.method).toBe('GET');
    expect(d.routes.state.method).toBe('GET'); // defaulted
    expect(d.routes.fetch.labels).toEqual([
      { kind: 'key', key: 'labels' },
      { kind: 'wildcard' },
      { kind: 'key', key: 'name' },
    ]);
  });

  it('honors authHeader / authScheme overrides (GitLab PRIVATE-TOKEN, no prefix)', () => {
    const d = parseRestWorkItemDescriptor({ ...valid, authHeader: 'PRIVATE-TOKEN', authScheme: '' });
    expect(d.authHeader).toBe('PRIVATE-TOKEN');
    expect(d.authScheme).toBe('');
  });

  it('rejects a non-https baseUrl and a host not in allowedHosts', () => {
    expect(() => parseRestWorkItemDescriptor({ ...valid, baseUrl: 'http://gitlab.example.com/api' })).toThrow(/must be https/);
    expect(() => parseRestWorkItemDescriptor({ ...valid, baseUrl: 'https://evil.example.com/api' })).toThrow(
      /not in allowedHosts/,
    );
  });

  it('rejects a path with a scheme/authority or an unknown placeholder', () => {
    const withPath = (path: string) => ({ ...valid, routes: { ...valid.routes, fetch: { ...valid.routes.fetch, path } } });
    expect(() => parseRestWorkItemDescriptor(withPath('projects/x'))).toThrow(/must start with "\/"/);
    expect(() => parseRestWorkItemDescriptor(withPath('//evil.com/x'))).toThrow(/no scheme or authority/);
    expect(() => parseRestWorkItemDescriptor(withPath('/x/{bogus}'))).toThrow(/unknown placeholder \{bogus\}/);
  });

  it('rejects missing required fields, a reserved authEnv, and a malformed allowedHosts entry', () => {
    expect(() => parseRestWorkItemDescriptor({ ...valid, allowedHosts: undefined })).toThrow(
      /invalid rest work-item provider/,
    );
    expect(() => parseRestWorkItemDescriptor({ ...valid, authEnv: 'GH_TOKEN' })).toThrow(/reserved/);
    expect(() => parseRestWorkItemDescriptor({ ...valid, baseUrl: 'https://h/api', allowedHosts: ['https://h'] })).toThrow(
      /bare host/,
    );
  });
});

describe('assertAllowedHost', () => {
  it('passes https allowlisted hosts and rejects http or off-allowlist hosts', () => {
    expect(() => assertAllowedHost('https://h.com/x', ['h.com'])).not.toThrow();
    expect(() => assertAllowedHost('http://h.com/x', ['h.com'])).toThrow(/must be https/);
    expect(() => assertAllowedHost('https://evil.com/x', ['h.com'])).toThrow(/not in allowedHosts/);
  });

  it('isAllowedHost is the non-throwing predicate form (used by pagination)', () => {
    expect(isAllowedHost('https://h.com/x', ['h.com'])).toBe(true);
    expect(isAllowedHost('http://h.com/x', ['h.com'])).toBe(false); // not https
    expect(isAllowedHost('https://evil.com/x', ['h.com'])).toBe(false); // off-allowlist
    expect(isAllowedHost('not a url', ['h.com'])).toBe(false); // unparseable
  });
});

describe('parseRestChangeRequestDescriptor', () => {
  const valid = {
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
  const withoutPipeline = {
    ...valid,
    routes: { findForBranch: valid.routes.findForBranch, create: valid.routes.create, squashMerge: valid.routes.squashMerge },
  };

  it('compiles a valid descriptor with method defaults', () => {
    const d = parseRestChangeRequestDescriptor(valid);
    expect(d.routes.findForBranch.method).toBe('GET'); // defaulted
    expect(d.routes.create.method).toBe('POST');
    expect(d.routes.squashMerge.method).toBe('PUT');
    expect(d.routes.findForBranch.url).toEqual({
      segments: [{ kind: 'index', index: 0 }, { kind: 'key', key: 'web_url' }],
      transforms: [],
    });
    expect(d.routes.create.body).toEqual(valid.routes.create.body);
    expect(d.routes.pipelineStatus?.stateMap).toEqual({ success: 'success', failed: 'failure', running: 'pending' });
  });

  it('allows omitting the optional pipelineStatus route', () => {
    expect(parseRestChangeRequestDescriptor(withoutPipeline).routes.pipelineStatus).toBeUndefined();
  });

  it('rejects unknown placeholders in a body or a path', () => {
    const badBody = { ...valid, routes: { ...valid.routes, create: { ...valid.routes.create, body: { title: '{title}', x: '{bogus}' } } } };
    expect(() => parseRestChangeRequestDescriptor(badBody)).toThrow(/create body uses unknown placeholder \{bogus\}/);
    // {id} is not bound at create time (the MR does not exist yet).
    const badPath = { ...valid, routes: { ...valid.routes, create: { ...valid.routes.create, path: '/x/{id}' } } };
    expect(() => parseRestChangeRequestDescriptor(badPath)).toThrow(/unknown placeholder \{id\}/);
  });

  it('enforces https, the host allowlist, the credential guard, and required routes', () => {
    expect(() => parseRestChangeRequestDescriptor({ ...valid, baseUrl: 'http://gitlab.example.com/api' })).toThrow(/must be https/);
    expect(() => parseRestChangeRequestDescriptor({ ...valid, allowedHosts: ['evil.example.com'] })).toThrow(/not in allowedHosts/);
    expect(() => parseRestChangeRequestDescriptor({ ...valid, authEnv: 'GH_TOKEN' })).toThrow(/reserved/);
    const noCreate = { ...valid, routes: { findForBranch: valid.routes.findForBranch, squashMerge: valid.routes.squashMerge } };
    expect(() => parseRestChangeRequestDescriptor(noCreate)).toThrow(/invalid rest change-request provider/);
  });

  it('compiles an optional failingJobs route with pagination (step 2.5b)', () => {
    const withJobs = parseRestChangeRequestDescriptor({
      ...valid,
      routes: {
        ...valid.routes,
        failingJobs: {
          path: '/projects/{repo}/merge_requests/{id}/jobs?scope=failed',
          itemsPath: '$',
          map: [{ name: '$.name', logExcerpt: '$.failure_reason | default:' }],
          paginate: { next: { style: 'nextUrl', path: '$.links.next' } },
        },
      },
    });
    const route = withJobs.routes.failingJobs!;
    expect(route.method).toBe('GET'); // defaulted
    expect(route.itemsPath).toEqual([]); // "$" ⇒ no segments (the body itself)
    expect(route.item.name).toEqual({ segments: [{ kind: 'key', key: 'name' }], transforms: [] });
    expect(route.item.logExcerpt.transforms).toEqual([{ kind: 'default', value: '' }]);
    expect(route.paginate).toEqual({
      next: { style: 'nextUrl', path: [{ kind: 'key', key: 'links' }, { kind: 'key', key: 'next' }] },
      maxPages: 10, // defaulted
    });
  });

  it('accepts pageParam pagination and a route without paginate (single page)', () => {
    const pageParam = parseRestChangeRequestDescriptor({
      ...valid,
      routes: {
        ...valid.routes,
        failingJobs: {
          path: '/projects/{repo}/merge_requests/{id}/jobs',
          itemsPath: '$.jobs',
          map: [{ name: '$.name', logExcerpt: '$.reason' }],
          paginate: { next: { style: 'pageParam', param: 'page', start: 1 }, maxPages: 3 },
        },
      },
    });
    expect(pageParam.routes.failingJobs?.paginate).toEqual({
      next: { style: 'pageParam', param: 'page', start: 1 },
      maxPages: 3,
    });

    const single = parseRestChangeRequestDescriptor({
      ...valid,
      routes: {
        ...valid.routes,
        failingJobs: { path: '/x/{repo}/{id}/jobs', itemsPath: '$', map: [{ name: '$.name', logExcerpt: '$.reason' }] },
      },
    });
    expect(single.routes.failingJobs?.paginate).toBeUndefined();
  });

  it('rejects a failingJobs map that is not exactly one template, a wildcard itemsPath, or a bad placeholder', () => {
    const withJobs = (failingJobs: unknown) =>
      parseRestChangeRequestDescriptor({ ...valid, routes: { ...valid.routes, failingJobs } });
    expect(() =>
      withJobs({ path: '/x/{repo}/{id}', itemsPath: '$', map: [], paginate: undefined }),
    ).toThrow(/invalid rest change-request provider/);
    expect(() =>
      withJobs({ path: '/x/{repo}/{id}', itemsPath: '$.jobs[*]', map: [{ name: '$.n', logExcerpt: '$.r' }] }),
    ).toThrow(/failingJobs\.itemsPath must not contain \[\*\]/);
    // {branch} is not bound for failingJobs (only {repo}/{id}).
    expect(() =>
      withJobs({ path: '/x/{branch}', itemsPath: '$', map: [{ name: '$.n', logExcerpt: '$.r' }] }),
    ).toThrow(/unknown placeholder \{branch\}/);
  });
});

describe('parseCliChangeRequestDescriptor', () => {
  const valid = {
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
    },
  };

  it('compiles a valid descriptor (commands kept, scalar maps pre-parsed with transforms)', () => {
    const d = parseCliChangeRequestDescriptor(valid);
    expect(d.authEnv).toBe('GITLAB_TOKEN');
    expect(d.routes.create.command).toContain('{title}');
    expect(d.routes.findForBranch.url).toEqual({
      segments: [{ kind: 'index', index: 0 }, { kind: 'key', key: 'web_url' }],
      transforms: [],
    });
    expect(d.routes.create.number).toEqual({ segments: [{ kind: 'key', key: 'iid' }], transforms: [] });
    expect(d.routes.pipelineStatus).toBeUndefined();
    expect(d.routes.failingJobs).toBeUndefined();
  });

  it('compiles optional pipelineStatus + single-shot failingJobs (no paginate)', () => {
    const d = parseCliChangeRequestDescriptor({
      ...valid,
      routes: {
        ...valid.routes,
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
    });
    expect(d.routes.pipelineStatus?.status.transforms).toEqual([{ kind: 'lower' }]);
    expect(d.routes.pipelineStatus?.stateMap).toEqual({ success: 'success', failed: 'failure' });
    expect(d.routes.failingJobs?.itemsPath).toEqual([{ kind: 'key', key: 'jobs' }]);
    expect(d.routes.failingJobs?.item.logExcerpt.transforms).toEqual([{ kind: 'default', value: '(none)' }]);
  });

  it('rejects missing required routes, an unknown placeholder, and a reserved authEnv', () => {
    expect(() =>
      parseCliChangeRequestDescriptor({ type: 'cli', routes: { findForBranch: valid.routes.findForBranch } }),
    ).toThrow(/invalid cli change-request provider.*create/);
    // {id} is not bound at create time (the MR does not exist yet).
    expect(() =>
      parseCliChangeRequestDescriptor({
        ...valid,
        routes: { ...valid.routes, create: { ...valid.routes.create, command: ['glab', 'mr', 'create', '{id}'] } },
      }),
    ).toThrow(/unknown placeholder \{id\}/);
    expect(() => parseCliChangeRequestDescriptor({ ...valid, authEnv: 'GH_TOKEN' })).toThrow(/reserved/);
  });
});
