/**
 * Declarative provider drivers (DESIGN-declarative-providers.md).
 *
 * These turn a validated descriptor (provider-descriptor.ts) into a standard
 * provider implementation. This is kernel code interpreting project *data* — it
 * loads no project code (the rejected Option A/D), and the orchestrator keeps
 * all git/gh authority (a work-item provider only reads and posts best-effort
 * progress).
 *
 * Step 2a ships the `cli` work-item driver: it shells the project's forge CLI
 * through `capture()` with a SCOPED, one-credential env from `safeSubprocessEnv`
 * — so the CLI gets PATH/HOME plus its single named token and never GH_TOKEN or
 * any other ambient secret. The `rest` (HTTP) driver lands in step 2b.
 *
 * The import back to providers.ts is type-only (erased), so the runtime graph
 * stays acyclic: providers.ts → providers-rest-cli.ts → provider-descriptor.ts.
 */

import { spawnSync } from 'node:child_process';

import { capture, formatProgress, note, type Captured } from './exec.js';
import { safeSubprocessEnv } from './env.js';
import {
  assertAllowedHost,
  evalArray,
  evalScalar,
  type CliWorkItemDescriptor,
  type RestBase,
  type RestChangeRequestDescriptor,
  type RestWorkItemDescriptor,
} from './provider-descriptor.js';
import type {
  ChangeRequestProvider,
  CreateChangeRequestInput,
  CreateChangeRequestResult,
  OperationResult,
  PipelineStatus,
  ProviderContext,
  WorkItemProvider,
} from './providers.js';

type CaptureFn = (cmd: readonly string[], opts?: { env?: Record<string, string> }) => Captured;

function substituteArgv(command: readonly string[], vars: Record<string, string>): string[] {
  return command.map((token) =>
    token.replace(/\{(\w+)\}/g, (whole, name: string) => {
      const value = vars[name];
      return value !== undefined ? value : whole;
    }),
  );
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return null;
  }
}

/**
 * Build a declarative `cli` WorkItemProvider from a validated descriptor.
 *
 * The scoped env is rebuilt per call (cheap; reads the current parent env). It
 * grants only the base allowlist plus the descriptor's one `authEnv`, with
 * GH_TOKEN withheld (`allowGhToken: false`) and deny-prefixed keys dropped by
 * `safeSubprocessEnv`. `captureFn` is an injectable seam for tests; production
 * uses the real synchronous `capture`.
 */
export function createCliWorkItemProvider(
  descriptor: CliWorkItemDescriptor,
  captureFn: CaptureFn = capture,
): WorkItemProvider {
  const scopedEnv = (): Record<string, string> =>
    safeSubprocessEnv({ allowGhToken: false, extraAllow: descriptor.authEnv ? [descriptor.authEnv] : [] });

  const run = (command: readonly string[], vars: Record<string, string>): Captured =>
    captureFn(substituteArgv(command, vars), { env: scopedEnv() });

  return {
    fetch: (ctx: ProviderContext, id) => {
      const result = run(descriptor.routes.fetch.command, { id: String(id), repo: ctx.repo });
      if (result.returncode !== 0) {
        return null;
      }
      const data = parseJson(result.stdout);
      if (data === null) {
        return null;
      }
      return {
        title: evalScalar(data, descriptor.routes.fetch.title),
        body: evalScalar(data, descriptor.routes.fetch.body),
        labels: evalArray(data, descriptor.routes.fetch.labels),
      };
    },
    state: (ctx: ProviderContext, id) => {
      const result = run(descriptor.routes.state.command, { id: String(id), repo: ctx.repo });
      if (result.returncode !== 0) {
        return 'UNKNOWN';
      }
      const data = parseJson(result.stdout);
      if (data === null) {
        return 'UNKNOWN';
      }
      return evalScalar(data, descriptor.routes.state.state) || 'UNKNOWN';
    },
    postProgress: (ctx: ProviderContext, id, adwId, phase, message) => {
      const route = descriptor.routes.postProgress;
      if (!route) {
        return;
      }
      const result = run(route.command, {
        id: String(id),
        repo: ctx.repo,
        body: formatProgress(adwId, phase, message),
      });
      if (result.returncode !== 0) {
        note(`could not post progress comment for #${id} (${phase})`);
      }
    },
    assignSelf: (ctx: ProviderContext, id) => {
      const route = descriptor.routes.assignSelf;
      if (!route) {
        return;
      }
      run(route.command, { id: String(id), repo: ctx.repo });
    },
    setStatus: (ctx: ProviderContext, id, status) => {
      const route = descriptor.routes.setStatus;
      if (!route) {
        return;
      }
      run(route.command, { id: String(id), repo: ctx.repo, status });
    },
  };
}

// ── rest (HTTP) work-item driver ─────────────────────────────────────────────

/** A single HTTP request the kernel performs on a provider's behalf. */
export interface RestRequest {
  method: string;
  url: string;
  /** Env-var NAME holding the token; resolved to a value only inside the helper. */
  authEnv: string;
  authHeader: string;
  authScheme: string;
  timeoutMs: number;
  /** Optional JSON request body (already placeholder-substituted). */
  body?: unknown;
}

/** The helper's reply: an HTTP status + body text, or a transport-level error. */
export interface RestResponse {
  status: number;
  body: string;
  error?: string;
}

/** Synchronous HTTP transport seam (injectable for tests); receives the scoped env. */
export type RestTransport = (req: RestRequest, env: Record<string, string>) => RestResponse;

const REST_TIMEOUT_MS = 15000;

// Kernel-owned one-shot HTTP helper, run as `node -e <this>` with a SCOPED env
// and the request on stdin. The token is read from the child's own env by NAME
// (never argv); only this fixed kernel script + the request reach the child.
// CommonJS `-e` has no top-level await → wrapped in an async IIFE. No project code.
const REST_FETCH_SCRIPT = `(async () => {
  const { readFileSync } = require('node:fs');
  let req;
  try { req = JSON.parse(readFileSync(0, 'utf8')); }
  catch { process.stdout.write('{"status":0,"body":"","error":"invalid request"}'); return; }
  const headers = { Accept: 'application/json' };
  const token = req.authEnv ? process.env[req.authEnv] : '';
  if (token) headers[req.authHeader] = req.authScheme ? (req.authScheme + ' ' + token) : token;
  const init = { method: req.method, headers, signal: AbortSignal.timeout(req.timeoutMs) };
  if (req.body !== undefined && req.body !== null) { headers['content-type'] = 'application/json'; init.body = JSON.stringify(req.body); }
  try {
    const res = await fetch(req.url, init);
    const body = await res.text();
    process.stdout.write(JSON.stringify({ status: res.status, body }));
  } catch (e) {
    process.stdout.write(JSON.stringify({ status: 0, body: '', error: String((e && e.message) || e) }));
  }
})();`;

/** Default transport: spawn the kernel helper synchronously with the scoped env. */
export const restTransportViaNode: RestTransport = (req, env) => {
  const result = spawnSync(process.execPath, ['-e', REST_FETCH_SCRIPT], {
    input: JSON.stringify(req),
    env,
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.status !== 0 || !result.stdout) {
    return { status: 0, body: '', error: result.stderr || 'rest helper failed' };
  }
  try {
    return JSON.parse(result.stdout) as RestResponse;
  } catch {
    return { status: 0, body: '', error: 'unparseable rest helper output' };
  }
};

function substitutePath(path: string, vars: Record<string, string>): string {
  return path.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const value = vars[name];
    return value !== undefined ? encodeURIComponent(value) : whole;
  });
}

/** Deep placeholder substitution into a JSON body template's string leaves (NOT url-encoded). */
function substituteBody(template: unknown, vars: Record<string, string>): unknown {
  if (typeof template === 'string') {
    return template.replace(/\{(\w+)\}/g, (whole, name: string) => {
      const value = vars[name];
      return value !== undefined ? value : whole;
    });
  }
  if (Array.isArray(template)) {
    return template.map((item) => substituteBody(item, vars));
  }
  if (template !== null && typeof template === 'object') {
    const out: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(template)) {
      out[key] = substituteBody(value, vars);
    }
    return out;
  }
  return template;
}

/**
 * Shared rest requester for both declarative roles. Resolves `baseUrl + path`
 * (placeholders percent-encoded so they cannot change the host), RE-asserts the
 * host is allowlisted + https (defense in depth over the parse-time check),
 * substitutes any JSON body template (passed explicitly — a route's own `body`
 * field would collide with the work-item map's `body`), and runs the request
 * through `transport` with a scoped one-credential env (GH_TOKEN withheld).
 */
function makeRestRequester(base: RestBase, transport: RestTransport) {
  const scopedEnv = (): Record<string, string> =>
    safeSubprocessEnv({ allowGhToken: false, extraAllow: [base.authEnv] });
  return (
    route: { method: string; path: string },
    vars: Record<string, string>,
    bodyTemplate?: Record<string, unknown>,
  ): RestResponse => {
    const url = base.baseUrl + substitutePath(route.path, vars);
    assertAllowedHost(url, base.allowedHosts);
    return transport(
      {
        method: route.method,
        url,
        authEnv: base.authEnv,
        authHeader: base.authHeader,
        authScheme: base.authScheme,
        timeoutMs: REST_TIMEOUT_MS,
        body: bodyTemplate !== undefined ? substituteBody(bodyTemplate, vars) : undefined,
      },
      scopedEnv(),
    );
  };
}

const restOk = (res: RestResponse): boolean => !res.error && res.status >= 200 && res.status < 300;

/**
 * Build a declarative `rest` WorkItemProvider from a validated descriptor.
 * `transport` is an injectable seam; production spawns the kernel helper.
 *
 * Step 2b ships read routes only (`fetch`/`state`); the write methods are no-ops
 * here pending request-body templating (used by the change-request provider).
 */
export function createRestWorkItemProvider(
  descriptor: RestWorkItemDescriptor,
  transport: RestTransport = restTransportViaNode,
): WorkItemProvider {
  const request = makeRestRequester(descriptor, transport);
  const ok = restOk;

  return {
    fetch: (ctx: ProviderContext, id) => {
      const res = request(descriptor.routes.fetch, { id: String(id), repo: ctx.repo });
      if (!ok(res)) {
        return null;
      }
      const data = parseJson(res.body);
      if (data === null) {
        return null;
      }
      return {
        title: evalScalar(data, descriptor.routes.fetch.title),
        body: evalScalar(data, descriptor.routes.fetch.body),
        labels: evalArray(data, descriptor.routes.fetch.labels),
      };
    },
    state: (ctx: ProviderContext, id) => {
      const res = request(descriptor.routes.state, { id: String(id), repo: ctx.repo });
      if (!ok(res)) {
        return 'UNKNOWN';
      }
      const data = parseJson(res.body);
      if (data === null) {
        return 'UNKNOWN';
      }
      return evalScalar(data, descriptor.routes.state.state) || 'UNKNOWN';
    },
    postProgress: () => {},
    assignSelf: () => {},
    setStatus: () => {},
  };
}

/**
 * Build a declarative `rest` ChangeRequestProvider from a validated descriptor.
 *
 * The merge-authorized path. `create`/`squashMerge` carry a templated JSON body;
 * every route — including `squashMerge` — goes through the same scoped
 * one-credential env + host-allowlist + https checks. The orchestrator still
 * owns the gating (it calls `squashMerge` only after the review/CI gates pass)
 * and all git; this provider only issues the forge's own API calls. `transport`
 * is an injectable seam.
 */
export function createRestChangeRequestProvider(
  descriptor: RestChangeRequestDescriptor,
  transport: RestTransport = restTransportViaNode,
): ChangeRequestProvider {
  const request = makeRestRequester(descriptor, transport);
  const ok = restOk;
  const routes = descriptor.routes;

  return {
    findForBranch: (ctx: ProviderContext, branch) => {
      const res = request(routes.findForBranch, { repo: ctx.repo, branch });
      if (!ok(res)) {
        return null;
      }
      const data = parseJson(res.body);
      if (data === null) {
        return null;
      }
      return evalScalar(data, routes.findForBranch.url) || null;
    },
    create: (ctx: ProviderContext, input: CreateChangeRequestInput): CreateChangeRequestResult => {
      const res = request(
        routes.create,
        { repo: ctx.repo, branch: input.branch, base: input.base, title: input.title, body: input.body },
        routes.create.body,
      );
      if (!ok(res)) {
        return { id: null, number: null, url: null, error: res.error ?? `create failed (status ${res.status})` };
      }
      const data = parseJson(res.body);
      if (data === null) {
        return { id: null, number: null, url: null, error: 'unparseable create response' };
      }
      const numberText = evalScalar(data, routes.create.number);
      const number = numberText !== '' && Number.isFinite(Number(numberText)) ? Number(numberText) : null;
      const url = evalScalar(data, routes.create.url) || null;
      const id = number !== null ? String(number) : url;
      return { id, number, url, error: null };
    },
    pipelineStatus: (ctx: ProviderContext, id): PipelineStatus => {
      const route = routes.pipelineStatus;
      if (!route) {
        // No pipeline route configured ⇒ nothing to gate on (treated as green).
        return { state: 'none', failingJobs: [] };
      }
      const res = request(route, { repo: ctx.repo, id: String(id) });
      if (!ok(res)) {
        return { state: 'unknown', failingJobs: [] };
      }
      const data = parseJson(res.body);
      if (data === null) {
        return { state: 'unknown', failingJobs: [] };
      }
      const forgeStatus = evalScalar(data, route.status);
      return { state: route.stateMap[forgeStatus] ?? 'unknown', failingJobs: [] };
    },
    squashMerge: (ctx: ProviderContext, id): OperationResult => {
      const res = request(routes.squashMerge, { repo: ctx.repo, id: String(id) }, routes.squashMerge.body);
      if (!ok(res)) {
        return { ok: false, error: res.error ?? `merge failed (status ${res.status})` };
      }
      return { ok: true, error: null };
    },
  };
}
