/**
 * Real `restTransportViaNode` HTTP coverage (GitHub issue #26).
 *
 * Every other rest test injects a FAKE transport, so the actually-shipped HTTP
 * path — `spawnSync` the kernel helper, marshal the request over stdin, read the
 * token by NAME from the child's scoped env, `fetch()` under an
 * `AbortSignal.timeout`, JSON round-trip the reply — had zero automated coverage.
 * These tests drive the real transport directly against a loopback HTTP server.
 *
 * CRITICAL — why the server is a FORKED child, not in-process:
 * `restTransportViaNode` uses `spawnSync`, which blocks this thread's event loop
 * until the helper child exits. A loopback `http.Server` in this same process
 * would be frozen during that block, the helper's `fetch` would never be
 * accepted, and the request would hang to its timeout (a deadlock). Hosting the
 * server in a `child_process.fork` gives it an independent event loop that keeps
 * running while `spawnSync` blocks us. See test/helpers/loopback-server.mjs and
 * DESIGN-declarative-providers.md §12.2. Do not move the server in-process.
 */

import { fork, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';

import { afterEach, describe, expect, it } from 'vitest';

import { assertAllowedHost, isAllowedHost, parseRestWorkItemDescriptor } from '../src/provider-descriptor.js';
import { restTransportViaNode, type RestResponse } from '../src/providers-rest-cli.js';
import { safeSubprocessEnv } from '../src/env.js';
import { withScopedEnv } from './helpers.js';

type LoopbackMode = 'echo' | 'status404' | 'hang';

interface CapturedRequest {
  method?: string;
  url?: string;
  headers: Record<string, string | string[] | undefined>;
  body: string;
}

interface Loopback {
  /** http://127.0.0.1:<ephemeral-port> */
  origin: string;
  port: number;
  /** Captured requests, queried over IPC AFTER the (sync) transport call returns. */
  requests(): Promise<CapturedRequest[]>;
  /** Idempotent: destroys held sockets, closes the server, awaits child exit. */
  close(): Promise<void>;
}

// Track open loopbacks so a failed assertion before close() never orphans a fork.
const openLoopbacks: Loopback[] = [];

afterEach(async () => {
  while (openLoopbacks.length > 0) {
    const lb = openLoopbacks.pop();
    if (lb) {
      await lb.close();
    }
  }
});

async function startLoopback(mode: LoopbackMode = 'echo'): Promise<Loopback> {
  const child: ChildProcess = fork(new URL('./helpers/loopback-server.mjs', import.meta.url), [], {
    // The fork is a plain test fixture, not a runner child, so the secret
    // boundary does not apply here (lint:env scans src/ only). It needs the
    // parent PATH to find node; LOOPBACK_MODE selects the response behavior.
    env: { ...process.env, LOOPBACK_MODE: mode },
    stdio: ['ignore', 'inherit', 'inherit', 'ipc'],
  });

  const [first] = (await once(child, 'message')) as [{ type: string; port: number }];
  if (first.type !== 'listening') {
    throw new Error(`loopback: unexpected first message "${first.type}"`);
  }
  const port = first.port;

  let closed = false;
  const lb: Loopback = {
    origin: `http://127.0.0.1:${port}`,
    port,
    requests: async () => {
      child.send({ type: 'requests' });
      const [reply] = (await once(child, 'message')) as [{ type: string; items: CapturedRequest[] }];
      return reply.items;
    },
    close: async () => {
      if (closed) {
        return;
      }
      closed = true;
      child.send({ type: 'close' });
      await once(child, 'exit');
    },
  };
  openLoopbacks.push(lb);
  return lb;
}

describe('restTransportViaNode (real HTTP loopback)', () => {
  it('GET round-trips through the real transport, reading the auth token by name', async () => {
    const lb = await startLoopback('echo');
    let res: RestResponse | undefined;
    withScopedEnv({ FORGE_TOKEN: 'tok-123', GH_TOKEN: 'gh-secret' }, () => {
      const env = safeSubprocessEnv({ allowGhToken: false, extraAllow: ['FORGE_TOKEN'] });
      expect(env['GH_TOKEN']).toBeUndefined(); // production scoping: GitHub authority withheld
      expect(env['FORGE_TOKEN']).toBe('tok-123');
      res = restTransportViaNode(
        {
          method: 'GET',
          url: `${lb.origin}/issues/42`,
          authEnv: 'FORGE_TOKEN',
          authHeader: 'Authorization',
          authScheme: 'Bearer',
          timeoutMs: 5000,
        },
        env,
      );
    });
    const [req] = await lb.requests();

    expect(res?.status).toBe(200);
    expect(res?.error).toBeUndefined();
    expect(JSON.parse(res?.body ?? '{}')).toMatchObject({ ok: true });
    expect(req?.method).toBe('GET');
    expect(req?.url).toBe('/issues/42');
    // The token was resolved by NAME inside the spawned child, never via argv.
    expect(req?.headers['authorization']).toBe('Bearer tok-123');
    expect(req?.headers['accept']).toBe('application/json');
  });

  it('POST sends a JSON body with content-type and round-trips it', async () => {
    const lb = await startLoopback('echo');
    let res: RestResponse | undefined;
    withScopedEnv({ FORGE_TOKEN: 'tok' }, () => {
      const env = safeSubprocessEnv({ allowGhToken: false, extraAllow: ['FORGE_TOKEN'] });
      res = restTransportViaNode(
        {
          method: 'POST',
          url: `${lb.origin}/issues/42/notes`,
          authEnv: 'FORGE_TOKEN',
          authHeader: 'Authorization',
          authScheme: 'Bearer',
          timeoutMs: 5000,
          body: { state_event: 'Done' },
        },
        env,
      );
    });
    const [req] = await lb.requests();

    expect(res?.status).toBe(200);
    expect(req?.method).toBe('POST');
    expect(req?.headers['content-type']).toBe('application/json');
    expect(JSON.parse(req?.body ?? '{}')).toEqual({ state_event: 'Done' });
    // The server echoes the raw request body back; the response round-trips.
    expect(JSON.parse(res?.body ?? '{}').echoBody).toBe(JSON.stringify({ state_event: 'Done' }));
  });

  it('omits Authorization when the named env var is unset', async () => {
    const lb = await startLoopback('echo');
    // No FORGE_TOKEN in the scoped env ⇒ the child's `if (token)` branch is false.
    const env = safeSubprocessEnv({ allowGhToken: false, extraAllow: ['FORGE_TOKEN'] });
    expect(env['FORGE_TOKEN']).toBeUndefined();
    const res = restTransportViaNode(
      {
        method: 'GET',
        url: `${lb.origin}/issues/1`,
        authEnv: 'FORGE_TOKEN',
        authHeader: 'Authorization',
        authScheme: 'Bearer',
        timeoutMs: 5000,
      },
      env,
    );
    const [req] = await lb.requests();

    expect(res.status).toBe(200);
    expect(req?.headers['authorization']).toBeUndefined();
  });

  it('relays a non-2xx status and body verbatim (no error)', async () => {
    const lb = await startLoopback('status404');
    const env = safeSubprocessEnv({ allowGhToken: false, extraAllow: ['FORGE_TOKEN'] });
    const res = restTransportViaNode(
      {
        method: 'GET',
        url: `${lb.origin}/missing`,
        authEnv: 'FORGE_TOKEN',
        authHeader: 'Authorization',
        authScheme: 'Bearer',
        timeoutMs: 5000,
      },
      env,
    );

    expect(res.status).toBe(404);
    expect(res.body).toBe('{"message":"nope"}');
    expect(res.error).toBeUndefined();
  });

  it('aborts via AbortSignal.timeout when the server never responds', async () => {
    const lb = await startLoopback('hang');
    const env = safeSubprocessEnv({ allowGhToken: false, extraAllow: ['FORGE_TOKEN'] });
    const res = restTransportViaNode(
      {
        method: 'GET',
        url: `${lb.origin}/slow`,
        authEnv: 'FORGE_TOKEN',
        authHeader: 'Authorization',
        authScheme: 'Bearer',
        timeoutMs: 250, // comfortably above loopback latency, still fast
      },
      env,
    );

    expect(res.status).toBe(0);
    expect(res.body).toBe('');
    // Abort wording varies by Node version ("operation was aborted" / "timeout").
    expect(res.error ?? '').toMatch(/abort|timeout/i);
  });

  it('surfaces a transport error on connection refused', async () => {
    const lb = await startLoopback('echo');
    const { origin } = lb;
    await lb.close(); // free the port, then aim at it ⇒ ECONNREFUSED
    const env = safeSubprocessEnv({ allowGhToken: false, extraAllow: ['FORGE_TOKEN'] });
    const res = restTransportViaNode(
      {
        method: 'GET',
        url: `${origin}/x`,
        authEnv: 'FORGE_TOKEN',
        authHeader: 'Authorization',
        authScheme: 'Bearer',
        timeoutMs: 5000,
      },
      env,
    );

    expect(res.status).toBe(0);
    expect(res.error).toBeTruthy();
  });

  it('the https/allowlist guard gates the real transport (no egress to a non-https target)', async () => {
    const lb = await startLoopback('echo');
    // Pointing a descriptor at the loopback over plain http must be rejected at
    // parse time, before any provider/requester can call the real transport.
    expect(() =>
      parseRestWorkItemDescriptor({
        type: 'rest',
        baseUrl: lb.origin, // http://127.0.0.1:<port> — non-https
        allowedHosts: [`127.0.0.1:${lb.port}`],
        authEnv: 'FORGE_TOKEN',
        routes: {
          fetch: { path: '/x', map: { title: '$.t', body: '$.b', labels: '$.l[*]' } },
          state: { path: '/x', map: { state: '$.s' } },
        },
      }),
    ).toThrow(/must be https/);

    // The guard fired before any request reached the loopback server.
    expect(await lb.requests()).toHaveLength(0);

    // And the standalone guard the requester re-asserts behaves on loopback
    // shapes (complements provider-descriptor.test.ts's host-string cases).
    const allow = [`127.0.0.1:${lb.port}`];
    expect(() => assertAllowedHost(`https://127.0.0.1:${lb.port}/x`, allow)).not.toThrow();
    expect(() => assertAllowedHost(`http://127.0.0.1:${lb.port}/x`, allow)).toThrow(/must be https/);
    expect(() => assertAllowedHost(`https://evil.test/x`, allow)).toThrow(/not in allowedHosts/);
    expect(isAllowedHost(`https://127.0.0.1:${lb.port}/x`, allow)).toBe(true);
    expect(isAllowedHost(`http://127.0.0.1:${lb.port}/x`, allow)).toBe(false);
  });
});
