import { describe, expect, it } from 'vitest';

import {
  assertAllowedHost,
  evalArray,
  evalScalar,
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
    expect(d.routes.fetch.title).toEqual([{ kind: 'key', key: 'title' }]);
    expect(d.routes.fetch.labels).toEqual([
      { kind: 'key', key: 'labels' },
      { kind: 'wildcard' },
      { kind: 'key', key: 'name' },
    ]);
    expect(d.routes.state.state).toEqual([{ kind: 'key', key: 'state' }]);
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
    for (const authEnv of ['GH_TOKEN', 'GH_BIN', 'MX_AGENT_YES', 'MATRIX_TOKEN', 'ANTHROPIC_API_KEY', 'OPENAI_API_KEY']) {
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
    expect(d.routes.findForBranch.url).toEqual([{ kind: 'index', index: 0 }, { kind: 'key', key: 'web_url' }]);
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
});
