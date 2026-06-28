/**
 * Declarative work-item provider descriptors (DESIGN-declarative-providers.md).
 *
 * A project can back its work items with a non-GitHub system by *describing*
 * the provider in `.adw/config.json` — command templates plus field mappings —
 * instead of shipping code. This module is the lone interpreter of that data:
 * it validates the descriptor, compiles the response-mapping mini-language, and
 * enforces the credential guard. It loads no project code and performs no I/O
 * (the driver in providers-rest-cli.ts does the effectful work); it is the
 * declarative analogue of schema-override.ts.
 *
 * Decoupled by construction: this file imports only zod, errors, env (for the
 * secret-boundary constants), and the WorkItemContext *type* — never config.ts
 * or providers.ts — so the dependency graph stays acyclic.
 */

import { z } from 'zod';

import { AdwError } from './errors.js';
import { ENV_DENY_PREFIXES, RUNNER_ENV_ALLOW } from './env.js';
import type { CiState } from './git.js';

// ── Response-mapping mini-language ───────────────────────────────────────────
// A deliberately under-powered subset of JSONPath — enough to map every real
// forge issue shape, trivially safe to evaluate (a pure data walk, no eval, no
// dependency). Grammar: `$` then a sequence of `.key`, `[index]`, or one `[*]`.

export type PathSegment = { kind: 'key'; key: string } | { kind: 'index'; index: number } | { kind: 'wildcard' };

const SEGMENT_RE = /^(?:\.([A-Za-z0-9_]+)|\[(\d+)\]|\[\*\])/;

/** Parse a `$.a.b[*].c` expression into segments; throws AdwError on bad grammar. */
export function parsePath(expr: string): PathSegment[] {
  if (!expr.startsWith('$')) {
    throw new AdwError(`invalid map path "${expr}": must start with $`);
  }
  let rest = expr.slice(1);
  const segments: PathSegment[] = [];
  while (rest.length > 0) {
    const m = SEGMENT_RE.exec(rest);
    if (m === null) {
      throw new AdwError(`invalid map path "${expr}" near "${rest}"`);
    }
    if (m[1] !== undefined) {
      segments.push({ kind: 'key', key: m[1] });
    } else if (m[2] !== undefined) {
      segments.push({ kind: 'index', index: Number(m[2]) });
    } else {
      segments.push({ kind: 'wildcard' });
    }
    rest = rest.slice(m[0].length);
  }
  if (segments.filter((s) => s.kind === 'wildcard').length > 1) {
    throw new AdwError(`invalid map path "${expr}": at most one [*] is allowed`);
  }
  return segments;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function coerceString(value: unknown): string {
  if (typeof value === 'string') {
    return value;
  }
  if (typeof value === 'number' || typeof value === 'boolean') {
    return String(value);
  }
  return '';
}

function step(current: unknown, seg: PathSegment): unknown {
  if (seg.kind === 'key') {
    return isRecord(current) ? current[seg.key] : undefined;
  }
  if (seg.kind === 'index') {
    return Array.isArray(current) ? current[seg.index] : undefined;
  }
  return undefined; // wildcard is handled by evalArray, never stepped here
}

/** Resolve a wildcard-free path to a single string (missing ⇒ ""), like the gh provider's coercion. */
export function evalScalar(data: unknown, segments: PathSegment[]): string {
  let current: unknown = data;
  for (const seg of segments) {
    current = step(current, seg);
    if (current === undefined || current === null) {
      return '';
    }
  }
  return coerceString(current);
}

/** Resolve a path containing one `[*]` to a string array (missing/non-array ⇒ []). */
export function evalArray(data: unknown, segments: PathSegment[]): string[] {
  const wildcardIndex = segments.findIndex((s) => s.kind === 'wildcard');
  if (wildcardIndex < 0) {
    return [];
  }
  const before = segments.slice(0, wildcardIndex);
  const after = segments.slice(wildcardIndex + 1);

  let container: unknown = data;
  for (const seg of before) {
    container = step(container, seg);
    if (container === undefined || container === null) {
      return [];
    }
  }
  if (!Array.isArray(container)) {
    return [];
  }
  return container.map((element) => {
    let value: unknown = element;
    for (const seg of after) {
      value = step(value, seg);
      if (value === undefined || value === null) {
        return '';
      }
    }
    return coerceString(value);
  });
}

/** Resolve a wildcard-free path to the array it points at (missing/non-array ⇒ []). */
export function evalItems(data: unknown, segments: PathSegment[]): unknown[] {
  let current: unknown = data;
  for (const seg of segments) {
    current = step(current, seg);
    if (current === undefined || current === null) {
      return [];
    }
  }
  return Array.isArray(current) ? current : [];
}

// ── Scalar transforms (step 2.5a) ────────────────────────────────────────────
// A scalar map value may carry a `|`-piped transform chain after its path, e.g.
// "$.pipeline.status | lower" or "$.iid | default:0". The vocabulary is a closed,
// eval-free set applied to the coerced string AFTER the data walk — pure data, no
// expressions, no user code. Array fields (labels) keep the bare `[*]` form;
// per-element transforms are a deliberately deferred concern.

export type Transform =
  | { kind: 'lower' }
  | { kind: 'upper' }
  | { kind: 'trim' }
  | { kind: 'default'; value: string };

/** A compiled scalar map value: a (wildcard-free) path plus its transform chain. */
export interface ScalarMapping {
  segments: PathSegment[];
  transforms: Transform[];
}

function parseTransform(part: string, label: string): Transform {
  const name = part.trim();
  if (name === 'lower' || name === 'upper' || name === 'trim') {
    return { kind: name };
  }
  // `default` needs an arg; `default:` (empty arg) is allowed and means "" (a no-op).
  if (name === 'default') {
    throw new AdwError(`${label} transform "default" requires an argument (e.g. default:0)`);
  }
  if (name.startsWith('default:')) {
    return { kind: 'default', value: name.slice('default:'.length) };
  }
  throw new AdwError(`${label} has unknown transform "${name}" (allowed: lower, upper, trim, default:<v>)`);
}

function applyTransform(value: string, transform: Transform): string {
  switch (transform.kind) {
    case 'lower':
      return value.toLowerCase();
    case 'upper':
      return value.toUpperCase();
    case 'trim':
      return value.trim();
    case 'default':
      return value === '' ? transform.value : value;
  }
}

/** Resolve a compiled scalar mapping: walk the path, then apply the transform chain. */
export function evalScalarMapping(data: unknown, mapping: ScalarMapping): string {
  let value = evalScalar(data, mapping.segments);
  for (const transform of mapping.transforms) {
    value = applyTransform(value, transform);
  }
  return value;
}

// ── Credential guard ─────────────────────────────────────────────────────────
// A declarative provider may name at most ONE credential, by env-var NAME. It
// must be the provider's own forge token — never the orchestrator's GitHub
// authority, a deny-prefixed secret, or a model credential. safeSubprocessEnv
// already drops deny-prefixed extraAllow keys; we ALSO reject them (and the
// reserved names) here so the descriptor fails loudly rather than silently.

const RESERVED_CREDENTIAL_NAMES = new Set<string>([
  'GH_TOKEN',
  'GH_BIN',
  ...Object.values(RUNNER_ENV_ALLOW).flat(),
]);

function assertSafeAuthEnv(authEnv: string): void {
  if (ENV_DENY_PREFIXES.some((prefix) => authEnv.startsWith(prefix))) {
    throw new AdwError(`provider authEnv "${authEnv}" matches a denied secret prefix and cannot be a provider credential`);
  }
  if (RESERVED_CREDENTIAL_NAMES.has(authEnv)) {
    throw new AdwError(`provider authEnv "${authEnv}" is reserved (GitHub or model credential) and cannot be a provider credential`);
  }
}

// ── Placeholder guard ────────────────────────────────────────────────────────
// Command templates may reference only the placeholders the orchestrator binds
// for that route. `capture()` runs with no shell, so a bound value is passed as
// one verbatim argv token — there is no word-splitting or injection through it.

const PLACEHOLDER_RE = /\{(\w+)\}/g;
const ALLOWED_PLACEHOLDERS = {
  fetch: ['id', 'repo'],
  state: ['id', 'repo'],
  postProgress: ['id', 'repo', 'body'],
  assignSelf: ['id', 'repo'],
  setStatus: ['id', 'repo', 'status'],
} as const;

function assertPlaceholders(command: readonly string[], allowed: readonly string[], route: string): void {
  for (const token of command) {
    for (const match of token.matchAll(PLACEHOLDER_RE)) {
      if (!allowed.includes(match[1] ?? '')) {
        throw new AdwError(
          `cli ${route} route command uses unknown placeholder {${match[1]}} (allowed: ${allowed.join(', ')})`,
        );
      }
    }
  }
}

// ── Descriptor schema (shape) ────────────────────────────────────────────────
// Strict shape validation via zod; the map values are raw path strings here and
// are compiled (and scalar/array-checked) below. `.strict()` makes a typo in a
// route or field a loud error rather than a silent drop.

// Shared by the cli and rest transports: a `fetch` route maps to the three
// WorkItemContext fields (title/body scalar, labels array); a `state` route
// maps to one scalar. The map values are raw path strings here, compiled below.
const fetchMapSchema = z
  .object({ title: z.string().min(1), body: z.string().min(1), labels: z.string().min(1) })
  .strict();
const stateMapSchema = z.object({ state: z.string().min(1) }).strict();

const rawCommandRoute = z.object({ command: z.array(z.string().min(1)).min(1) }).strict();
const rawFetchRoute = z.object({ command: z.array(z.string().min(1)).min(1), map: fetchMapSchema }).strict();
const rawStateRoute = z.object({ command: z.array(z.string().min(1)).min(1), map: stateMapSchema }).strict();

const RawCliDescriptorSchema = z
  .object({
    authEnv: z.string().min(1).optional(),
    routes: z
      .object({
        fetch: rawFetchRoute,
        state: rawStateRoute,
        postProgress: rawCommandRoute.optional(),
        assignSelf: rawCommandRoute.optional(),
        setStatus: rawCommandRoute.optional(),
      })
      .strict(),
  })
  .strict();

/** A validated, compiled cli work-item descriptor (paths pre-parsed once). */
export interface CliWorkItemDescriptor {
  authEnv?: string;
  routes: {
    fetch: { command: string[] } & FetchFieldMap;
    state: { command: string[]; state: ScalarMapping };
    postProgress?: { command: string[] };
    assignSelf?: { command: string[] };
    setStatus?: { command: string[] };
  };
}

function noWildcard(segments: PathSegment[], message: string): PathSegment[] {
  if (segments.some((s) => s.kind === 'wildcard')) {
    throw new AdwError(message);
  }
  return segments;
}

/** Compile a scalar map value: a `|`-separated path + transform chain (step 2.5a). */
function compileScalar(expr: string, label: string): ScalarMapping {
  const parts = expr.split('|');
  const pathExpr = (parts[0] ?? '').trim();
  return {
    segments: noWildcard(parsePath(pathExpr), `${label} must be a scalar path (no [*]): "${pathExpr}"`),
    transforms: parts.slice(1).map((part) => parseTransform(part, label)),
  };
}

/** Compile a wildcard-free path that locates an array container (e.g. paginate.itemsPath). */
function compileItemsPath(expr: string, label: string): PathSegment[] {
  return noWildcard(parsePath(expr), `${label} must not contain [*]: "${expr}"`);
}

function arrayPath(expr: string, label: string): PathSegment[] {
  const segments = parsePath(expr);
  if (!segments.some((s) => s.kind === 'wildcard')) {
    throw new AdwError(`${label} must be an array path (use [*]): "${expr}"`);
  }
  return segments;
}

/** Compiled `fetch` map: the three WorkItemContext fields (scalars carry transforms). */
export interface FetchFieldMap {
  title: ScalarMapping;
  body: ScalarMapping;
  labels: PathSegment[];
}

function compileFetchMap(map: { title: string; body: string; labels: string }): FetchFieldMap {
  return {
    title: compileScalar(map.title, 'fetch.map.title'),
    body: compileScalar(map.body, 'fetch.map.body'),
    labels: arrayPath(map.labels, 'fetch.map.labels'),
  };
}

function compileStatePath(map: { state: string }): ScalarMapping {
  return compileScalar(map.state, 'state.map.state');
}

/**
 * Validate + compile the `cli` work-item descriptor from the providers.workItems
 * config slice. Throws a loud AdwError on any shape, grammar, placeholder, or
 * credential violation — run-start fail-closed (so a --dry-run checks it too).
 */
export function parseCliWorkItemDescriptor(slice: unknown): CliWorkItemDescriptor {
  const obj = isRecord(slice) ? slice : {};
  const parsed = RawCliDescriptorSchema.safeParse({ authEnv: obj['authEnv'], routes: obj['routes'] });
  if (!parsed.success) {
    throw new AdwError(
      `invalid cli work-item provider: ${parsed.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`).join('; ')}`,
    );
  }
  const raw = parsed.data;
  if (raw.authEnv !== undefined) {
    assertSafeAuthEnv(raw.authEnv);
  }

  assertPlaceholders(raw.routes.fetch.command, ALLOWED_PLACEHOLDERS.fetch, 'fetch');
  assertPlaceholders(raw.routes.state.command, ALLOWED_PLACEHOLDERS.state, 'state');

  const routes: CliWorkItemDescriptor['routes'] = {
    fetch: { command: raw.routes.fetch.command, ...compileFetchMap(raw.routes.fetch.map) },
    state: { command: raw.routes.state.command, state: compileStatePath(raw.routes.state.map) },
  };

  if (raw.routes.postProgress) {
    assertPlaceholders(raw.routes.postProgress.command, ALLOWED_PLACEHOLDERS.postProgress, 'postProgress');
    routes.postProgress = { command: raw.routes.postProgress.command };
  }
  if (raw.routes.assignSelf) {
    assertPlaceholders(raw.routes.assignSelf.command, ALLOWED_PLACEHOLDERS.assignSelf, 'assignSelf');
    routes.assignSelf = { command: raw.routes.assignSelf.command };
  }
  if (raw.routes.setStatus) {
    assertPlaceholders(raw.routes.setStatus.command, ALLOWED_PLACEHOLDERS.setStatus, 'setStatus');
    routes.setStatus = { command: raw.routes.setStatus.command };
  }

  return { authEnv: raw.authEnv, routes };
}

// ── rest (HTTP) descriptors ──────────────────────────────────────────────────
// The same field mapping, over HTTP. The kernel performs each request
// (providers-rest-cli.ts) against an allowlisted https host, injecting one named
// credential as a header value; the descriptor stays pure data. Placeholders are
// percent-encoded into the path so they cannot alter the host. Work items and
// change requests share the rest "base" (host/credential) and differ in routes.

const restMethod = z.enum(['GET', 'POST', 'PUT', 'PATCH', 'DELETE']);

const restBaseFields = {
  baseUrl: z.string().min(1),
  allowedHosts: z.array(z.string().min(1)).min(1),
  authEnv: z.string().min(1),
  authHeader: z.string().min(1).optional(),
  authScheme: z.string().optional(),
};

/** The validated host/credential half shared by every rest descriptor. */
export interface RestBase {
  baseUrl: string;
  allowedHosts: string[];
  authEnv: string;
  /** Header carrying the credential (default 'Authorization'). */
  authHeader: string;
  /** Prefix before the token; '' ⇒ raw token (default 'Bearer'). */
  authScheme: string;
}

/** Assert a fully-resolved URL is https and its host is allowlisted; else throw. */
export function assertAllowedHost(url: string, allowedHosts: readonly string[]): void {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw new AdwError(`rest URL is not valid: "${url}"`);
  }
  if (parsed.protocol !== 'https:') {
    throw new AdwError(`rest URL must be https: "${url}"`);
  }
  if (!allowedHosts.includes(parsed.host)) {
    throw new AdwError(`rest URL host "${parsed.host}" is not in allowedHosts [${allowedHosts.join(', ')}]`);
  }
}

/**
 * Boolean form of {@link assertAllowedHost}. Pagination follows a next-page URL
 * that comes from the (attacker-influenceable) response, so it must STOP — not
 * throw — on an off-allowlist host; this predicate lets the loop end gracefully
 * while reusing the exact same https + allowlist check.
 */
export function isAllowedHost(url: string, allowedHosts: readonly string[]): boolean {
  try {
    assertAllowedHost(url, allowedHosts);
    return true;
  } catch {
    return false;
  }
}

function resolveRestBase(raw: {
  baseUrl: string;
  allowedHosts: string[];
  authEnv: string;
  authHeader?: string;
  authScheme?: string;
}): RestBase {
  assertSafeAuthEnv(raw.authEnv);
  for (const host of raw.allowedHosts) {
    if (host.includes('/') || host.includes(' ') || host.includes('://')) {
      throw new AdwError(`rest allowedHosts entry "${host}" must be a bare host[:port]`);
    }
  }
  assertAllowedHost(raw.baseUrl, raw.allowedHosts);
  return {
    baseUrl: raw.baseUrl,
    allowedHosts: raw.allowedHosts,
    authEnv: raw.authEnv,
    authHeader: raw.authHeader ?? 'Authorization',
    authScheme: raw.authScheme ?? 'Bearer',
  };
}

function assertRestPath(path: string, allowed: readonly string[], route: string): void {
  if (!path.startsWith('/')) {
    throw new AdwError(`rest ${route} path must start with "/": "${path}"`);
  }
  if (path.startsWith('//') || path.includes('://')) {
    throw new AdwError(`rest ${route} path must be a plain path (no scheme or authority): "${path}"`);
  }
  assertPlaceholders([path], allowed, route);
}

function collectBodyPlaceholders(template: unknown, found: Set<string>): void {
  if (typeof template === 'string') {
    for (const match of template.matchAll(PLACEHOLDER_RE)) {
      found.add(match[1] ?? '');
    }
  } else if (Array.isArray(template)) {
    for (const item of template) {
      collectBodyPlaceholders(item, found);
    }
  } else if (isRecord(template)) {
    for (const value of Object.values(template)) {
      collectBodyPlaceholders(value, found);
    }
  }
}

function assertBodyPlaceholders(body: unknown, allowed: readonly string[], route: string): void {
  const found = new Set<string>();
  collectBodyPlaceholders(body, found);
  for (const name of found) {
    if (!allowed.includes(name)) {
      throw new AdwError(`rest ${route} body uses unknown placeholder {${name}} (allowed: ${allowed.join(', ')})`);
    }
  }
}

function restParseError(kind: string, error: z.ZodError): AdwError {
  return new AdwError(`invalid rest ${kind} provider: ${error.issues.map((i) => `${i.path.join('.')}: ${i.message}`).join('; ')}`);
}

// ── rest work-item descriptor ────────────────────────────────────────────────

const WORK_ITEM_PLACEHOLDERS = ['id', 'repo'] as const;
// Write-route placeholder sets, mirroring the cli ALLOWED_PLACEHOLDERS above:
// postProgress binds the formatted comment body, setStatus the target status,
// assignSelf nothing beyond the item id/repo (REST has no universal "self").
const WI_POST_PROGRESS_PLACEHOLDERS = ['id', 'repo', 'body'] as const;
const WI_ASSIGN_PLACEHOLDERS = ['id', 'repo'] as const;
const WI_SET_STATUS_PLACEHOLDERS = ['id', 'repo', 'status'] as const;

const rawRestFetchRoute = z
  .object({ method: restMethod.default('GET'), path: z.string().min(1), map: fetchMapSchema })
  .strict();
const rawRestStateRoute = z
  .object({ method: restMethod.default('GET'), path: z.string().min(1), map: stateMapSchema })
  .strict();
// Optional write routes — each carries an optional templated JSON body (same
// shape as the change-request create/merge routes). Sensible method defaults:
// POST for postProgress/assignSelf, PUT for setStatus.
const rawRestPostProgressRoute = z
  .object({ method: restMethod.default('POST'), path: z.string().min(1), body: z.record(z.string(), z.unknown()).optional() })
  .strict();
const rawRestAssignSelfRoute = z
  .object({ method: restMethod.default('POST'), path: z.string().min(1), body: z.record(z.string(), z.unknown()).optional() })
  .strict();
const rawRestSetStatusRoute = z
  .object({ method: restMethod.default('PUT'), path: z.string().min(1), body: z.record(z.string(), z.unknown()).optional() })
  .strict();

const RawRestWorkItemSchema = z
  .object({
    ...restBaseFields,
    routes: z
      .object({
        fetch: rawRestFetchRoute,
        state: rawRestStateRoute,
        postProgress: rawRestPostProgressRoute.optional(),
        assignSelf: rawRestAssignSelfRoute.optional(),
        setStatus: rawRestSetStatusRoute.optional(),
      })
      .strict(),
  })
  .strict();

/** A validated, compiled rest work-item descriptor (paths/maps pre-parsed). */
export interface RestWorkItemDescriptor extends RestBase {
  routes: {
    fetch: { method: string; path: string } & FetchFieldMap;
    state: { method: string; path: string; state: ScalarMapping };
    postProgress?: { method: string; path: string; body?: Record<string, unknown> };
    assignSelf?: { method: string; path: string; body?: Record<string, unknown> };
    setStatus?: { method: string; path: string; body?: Record<string, unknown> };
  };
}

/**
 * Validate + compile the `rest` work-item descriptor. Throws a loud AdwError on
 * any shape, credential, host, https, path, or map-grammar violation — run-start
 * fail-closed (so a --dry-run checks it too). The host allowlist is checked here
 * against `baseUrl`; the driver re-checks every resolved URL defensively.
 */
export function parseRestWorkItemDescriptor(slice: unknown): RestWorkItemDescriptor {
  const obj = isRecord(slice) ? slice : {};
  const parsed = RawRestWorkItemSchema.safeParse({
    baseUrl: obj['baseUrl'],
    allowedHosts: obj['allowedHosts'],
    authEnv: obj['authEnv'],
    authHeader: obj['authHeader'],
    authScheme: obj['authScheme'],
    routes: obj['routes'],
  });
  if (!parsed.success) {
    throw restParseError('work-item', parsed.error);
  }
  const raw = parsed.data;
  const base = resolveRestBase(raw);
  assertRestPath(raw.routes.fetch.path, WORK_ITEM_PLACEHOLDERS, 'fetch');
  assertRestPath(raw.routes.state.path, WORK_ITEM_PLACEHOLDERS, 'state');
  const routes: RestWorkItemDescriptor['routes'] = {
    fetch: { method: raw.routes.fetch.method, path: raw.routes.fetch.path, ...compileFetchMap(raw.routes.fetch.map) },
    state: {
      method: raw.routes.state.method,
      path: raw.routes.state.path,
      state: compileStatePath(raw.routes.state.map),
    },
  };
  if (raw.routes.postProgress) {
    assertRestPath(raw.routes.postProgress.path, WI_POST_PROGRESS_PLACEHOLDERS, 'postProgress');
    if (raw.routes.postProgress.body !== undefined) {
      assertBodyPlaceholders(raw.routes.postProgress.body, WI_POST_PROGRESS_PLACEHOLDERS, 'postProgress');
    }
    routes.postProgress = {
      method: raw.routes.postProgress.method,
      path: raw.routes.postProgress.path,
      body: raw.routes.postProgress.body,
    };
  }
  if (raw.routes.assignSelf) {
    assertRestPath(raw.routes.assignSelf.path, WI_ASSIGN_PLACEHOLDERS, 'assignSelf');
    if (raw.routes.assignSelf.body !== undefined) {
      assertBodyPlaceholders(raw.routes.assignSelf.body, WI_ASSIGN_PLACEHOLDERS, 'assignSelf');
    }
    routes.assignSelf = {
      method: raw.routes.assignSelf.method,
      path: raw.routes.assignSelf.path,
      body: raw.routes.assignSelf.body,
    };
  }
  if (raw.routes.setStatus) {
    assertRestPath(raw.routes.setStatus.path, WI_SET_STATUS_PLACEHOLDERS, 'setStatus');
    if (raw.routes.setStatus.body !== undefined) {
      assertBodyPlaceholders(raw.routes.setStatus.body, WI_SET_STATUS_PLACEHOLDERS, 'setStatus');
    }
    routes.setStatus = {
      method: raw.routes.setStatus.method,
      path: raw.routes.setStatus.path,
      body: raw.routes.setStatus.body,
    };
  }
  return { ...base, routes };
}

/**
 * Run-start fail-closed guard shared by the declarative work-item factories. An
 * opt-in terminal board transition (`doneStatus`) is a loss-bearing write: the
 * operator has explicitly asked for it. If it is configured but the provider has
 * no `setStatus` route to honor it, the transition would be silently dropped
 * (the orchestrator swallows the no-op as best-effort) — so refuse to build the
 * provider instead of losing the write at runtime. Provider-agnostic: the values
 * are passed in, so this stays decoupled from config.ts.
 */
export function assertStatusTransitionRoutable(
  doneStatus: string | undefined,
  hasSetStatusRoute: boolean,
  kind: 'cli' | 'rest',
): void {
  if (doneStatus && !hasSetStatusRoute) {
    throw new AdwError(
      `workItems.doneStatus "${doneStatus}" is configured but the ${kind} work-item provider has no ` +
        `setStatus route; the terminal board transition would be silently dropped. ` +
        `Add a setStatus route or remove doneStatus.`,
    );
  }
}

// ── Pagination (step 2.5b) ───────────────────────────────────────────────────
// A list route (currently `failingJobs`) may span pages. The kernel fetches each
// page, extracts `itemsPath` items, finds the next page, and stops at `maxPages`
// (logged, never silent) — re-asserting the host allowlist on every followed URL
// (a next-page URL comes from the attacker-influenceable response). Two cursor
// styles only: `nextUrl` (next absolute URL from a body path) and `pageParam`
// (increment a query param until a page yields zero items). `Link`-header
// pagination is deferred (it would need response headers from the fetch helper).

export type PageCursor =
  | { style: 'nextUrl'; path: PathSegment[] }
  | { style: 'pageParam'; param: string; start: number };

export interface Paginate {
  next: PageCursor;
  maxPages: number;
}

const rawPageCursor = z.discriminatedUnion('style', [
  z.object({ style: z.literal('nextUrl'), path: z.string().min(1) }).strict(),
  z.object({ style: z.literal('pageParam'), param: z.string().min(1), start: z.number().int().default(1) }).strict(),
]);
const rawPaginate = z.object({ next: rawPageCursor, maxPages: z.number().int().positive().default(10) }).strict();

function compilePaginate(raw: z.infer<typeof rawPaginate>): Paginate {
  const next: PageCursor =
    raw.next.style === 'nextUrl'
      ? { style: 'nextUrl', path: compileItemsPath(raw.next.path, 'paginate.next.path') }
      : { style: 'pageParam', param: raw.next.param, start: raw.next.start };
  return { next, maxPages: raw.maxPages };
}

// ── rest change-request descriptor ───────────────────────────────────────────
// The merge-authorized path. `create`/`squashMerge` are write routes carrying a
// templated JSON body. `squashMerge` is the single most sensitive operation, so
// it is host-allowlisted + https + scoped-credential like every rest route, and
// the orchestrator still owns the GATING (it calls squashMerge only after the
// review/CI gates pass) and all git (branch/commit/push stay the `git` provider).
// A provider never receives GH_TOKEN or raw git/gh authority — the worst a
// descriptor can do is one templated request to its own allowlisted forge host
// with the user's own scoped forge token.

const CR_FIND_PLACEHOLDERS = ['repo', 'branch'] as const;
const CR_CREATE_PLACEHOLDERS = ['repo', 'branch', 'base', 'title', 'body'] as const;
const CR_ID_PLACEHOLDERS = ['repo', 'id'] as const;

const rawCrFindRoute = z
  .object({
    method: restMethod.default('GET'),
    path: z.string().min(1),
    map: z.object({ url: z.string().min(1) }).strict(),
  })
  .strict();
const rawCrCreateRoute = z
  .object({
    method: restMethod.default('POST'),
    path: z.string().min(1),
    body: z.record(z.string(), z.unknown()).optional(),
    map: z.object({ number: z.string().min(1), url: z.string().min(1) }).strict(),
  })
  .strict();
const rawCrMergeRoute = z
  .object({ method: restMethod.default('PUT'), path: z.string().min(1), body: z.record(z.string(), z.unknown()).optional() })
  .strict();
const rawCrPipelineRoute = z
  .object({
    method: restMethod.default('GET'),
    path: z.string().min(1),
    statusPath: z.string().min(1),
    stateMap: z.record(z.string(), z.enum(['success', 'failure', 'pending', 'none', 'unknown'])),
  })
  .strict();
// `failingJobs` (step 2.5b): a list route. `itemsPath` locates the jobs array on
// a page; `map` is a one-element array whose object templates each PipelineJob;
// `paginate` (optional) walks pages. It is fetched with the same {id} as
// pipelineStatus (the change-request id) — multi-step pipeline-id resolution is
// step-3 territory, not this primitive.
const rawCrFailingJobsRoute = z
  .object({
    method: restMethod.default('GET'),
    path: z.string().min(1),
    itemsPath: z.string().min(1),
    map: z.array(z.object({ name: z.string().min(1), logExcerpt: z.string().min(1) }).strict()).length(1),
    paginate: rawPaginate.optional(),
  })
  .strict();

const RawRestChangeRequestSchema = z
  .object({
    ...restBaseFields,
    routes: z
      .object({
        findForBranch: rawCrFindRoute,
        create: rawCrCreateRoute,
        squashMerge: rawCrMergeRoute,
        pipelineStatus: rawCrPipelineRoute.optional(),
        failingJobs: rawCrFailingJobsRoute.optional(),
      })
      .strict(),
  })
  .strict();

/** A validated, compiled rest change-request descriptor (paths/maps pre-parsed). */
export interface RestChangeRequestDescriptor extends RestBase {
  routes: {
    findForBranch: { method: string; path: string; url: ScalarMapping };
    create: { method: string; path: string; body?: Record<string, unknown>; number: ScalarMapping; url: ScalarMapping };
    squashMerge: { method: string; path: string; body?: Record<string, unknown> };
    pipelineStatus?: { method: string; path: string; status: ScalarMapping; stateMap: Record<string, CiState> };
    failingJobs?: {
      method: string;
      path: string;
      itemsPath: PathSegment[];
      item: { name: ScalarMapping; logExcerpt: ScalarMapping };
      paginate?: Paginate;
    };
  };
}

/**
 * Validate + compile the `rest` change-request descriptor. Same fail-closed
 * posture as the work-item path, plus request-body placeholder validation. The
 * `squashMerge` route is the merge-authorized one; it is bound by the same host
 * allowlist + https + scoped credential as every route.
 */
export function parseRestChangeRequestDescriptor(slice: unknown): RestChangeRequestDescriptor {
  const obj = isRecord(slice) ? slice : {};
  const parsed = RawRestChangeRequestSchema.safeParse({
    baseUrl: obj['baseUrl'],
    allowedHosts: obj['allowedHosts'],
    authEnv: obj['authEnv'],
    authHeader: obj['authHeader'],
    authScheme: obj['authScheme'],
    routes: obj['routes'],
  });
  if (!parsed.success) {
    throw restParseError('change-request', parsed.error);
  }
  const raw = parsed.data;
  const base = resolveRestBase(raw);
  const r = raw.routes;

  assertRestPath(r.findForBranch.path, CR_FIND_PLACEHOLDERS, 'findForBranch');
  assertRestPath(r.create.path, CR_CREATE_PLACEHOLDERS, 'create');
  if (r.create.body !== undefined) {
    assertBodyPlaceholders(r.create.body, CR_CREATE_PLACEHOLDERS, 'create');
  }
  assertRestPath(r.squashMerge.path, CR_ID_PLACEHOLDERS, 'squashMerge');
  if (r.squashMerge.body !== undefined) {
    assertBodyPlaceholders(r.squashMerge.body, CR_ID_PLACEHOLDERS, 'squashMerge');
  }

  const routes: RestChangeRequestDescriptor['routes'] = {
    findForBranch: {
      method: r.findForBranch.method,
      path: r.findForBranch.path,
      url: compileScalar(r.findForBranch.map.url, 'findForBranch.map.url'),
    },
    create: {
      method: r.create.method,
      path: r.create.path,
      body: r.create.body,
      number: compileScalar(r.create.map.number, 'create.map.number'),
      url: compileScalar(r.create.map.url, 'create.map.url'),
    },
    squashMerge: { method: r.squashMerge.method, path: r.squashMerge.path, body: r.squashMerge.body },
  };
  if (r.pipelineStatus !== undefined) {
    assertRestPath(r.pipelineStatus.path, CR_ID_PLACEHOLDERS, 'pipelineStatus');
    routes.pipelineStatus = {
      method: r.pipelineStatus.method,
      path: r.pipelineStatus.path,
      status: compileScalar(r.pipelineStatus.statusPath, 'pipelineStatus.statusPath'),
      stateMap: r.pipelineStatus.stateMap,
    };
  }
  if (r.failingJobs !== undefined) {
    assertRestPath(r.failingJobs.path, CR_ID_PLACEHOLDERS, 'failingJobs');
    const template = r.failingJobs.map[0]!;
    routes.failingJobs = {
      method: r.failingJobs.method,
      path: r.failingJobs.path,
      itemsPath: compileItemsPath(r.failingJobs.itemsPath, 'failingJobs.itemsPath'),
      item: {
        name: compileScalar(template.name, 'failingJobs.map.name'),
        logExcerpt: compileScalar(template.logExcerpt, 'failingJobs.map.logExcerpt'),
      },
      paginate: r.failingJobs.paginate ? compilePaginate(r.failingJobs.paginate) : undefined,
    };
  }
  return { ...base, routes };
}

// ── cli change-request descriptor ────────────────────────────────────────────
// The CLI symmetry of the rest change-request path: a project drives the change-
// request lifecycle through its forge CLI (`glab mr …`) by describing the command
// templates + field maps, instead of HTTP routes. Same field grammar (scalars
// carry transforms; `failingJobs` maps an array via `itemsPath` + a one-element
// item template — single-shot, since a CLI returns the whole list per invocation,
// so there is no `paginate` here). `squashMerge` is the merge-authorized route;
// it shares the scoped one-credential env (GH_TOKEN withheld) and the no-shell
// `capture()` boundary every cli route already uses (§8k) — the orchestrator
// keeps the gating and all git, and the provider never receives GH_TOKEN.

const rawCliCrFindRoute = z
  .object({ command: z.array(z.string().min(1)).min(1), map: z.object({ url: z.string().min(1) }).strict() })
  .strict();
const rawCliCrCreateRoute = z
  .object({
    command: z.array(z.string().min(1)).min(1),
    map: z.object({ number: z.string().min(1), url: z.string().min(1) }).strict(),
  })
  .strict();
const rawCliCrPipelineRoute = z
  .object({
    command: z.array(z.string().min(1)).min(1),
    statusPath: z.string().min(1),
    stateMap: z.record(z.string(), z.enum(['success', 'failure', 'pending', 'none', 'unknown'])),
  })
  .strict();
const rawCliCrFailingJobsRoute = z
  .object({
    command: z.array(z.string().min(1)).min(1),
    itemsPath: z.string().min(1),
    map: z.array(z.object({ name: z.string().min(1), logExcerpt: z.string().min(1) }).strict()).length(1),
  })
  .strict();

const RawCliChangeRequestSchema = z
  .object({
    authEnv: z.string().min(1).optional(),
    routes: z
      .object({
        findForBranch: rawCliCrFindRoute,
        create: rawCliCrCreateRoute,
        squashMerge: rawCommandRoute,
        pipelineStatus: rawCliCrPipelineRoute.optional(),
        failingJobs: rawCliCrFailingJobsRoute.optional(),
      })
      .strict(),
  })
  .strict();

/** A validated, compiled cli change-request descriptor (commands + maps pre-parsed). */
export interface CliChangeRequestDescriptor {
  authEnv?: string;
  routes: {
    findForBranch: { command: string[]; url: ScalarMapping };
    create: { command: string[]; number: ScalarMapping; url: ScalarMapping };
    squashMerge: { command: string[] };
    pipelineStatus?: { command: string[]; status: ScalarMapping; stateMap: Record<string, CiState> };
    failingJobs?: { command: string[]; itemsPath: PathSegment[]; item: { name: ScalarMapping; logExcerpt: ScalarMapping } };
  };
}

/**
 * Validate + compile the `cli` change-request descriptor from the
 * providers.changeRequests config slice. Throws a loud AdwError on any shape,
 * grammar, placeholder, or credential violation — run-start fail-closed (so a
 * --dry-run checks it too). Reuses the same CR placeholder sets as the rest path.
 */
export function parseCliChangeRequestDescriptor(slice: unknown): CliChangeRequestDescriptor {
  const obj = isRecord(slice) ? slice : {};
  const parsed = RawCliChangeRequestSchema.safeParse({ authEnv: obj['authEnv'], routes: obj['routes'] });
  if (!parsed.success) {
    throw new AdwError(
      `invalid cli change-request provider: ${parsed.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`).join('; ')}`,
    );
  }
  const raw = parsed.data;
  if (raw.authEnv !== undefined) {
    assertSafeAuthEnv(raw.authEnv);
  }
  const r = raw.routes;
  assertPlaceholders(r.findForBranch.command, CR_FIND_PLACEHOLDERS, 'findForBranch');
  assertPlaceholders(r.create.command, CR_CREATE_PLACEHOLDERS, 'create');
  assertPlaceholders(r.squashMerge.command, CR_ID_PLACEHOLDERS, 'squashMerge');

  const routes: CliChangeRequestDescriptor['routes'] = {
    findForBranch: { command: r.findForBranch.command, url: compileScalar(r.findForBranch.map.url, 'findForBranch.map.url') },
    create: {
      command: r.create.command,
      number: compileScalar(r.create.map.number, 'create.map.number'),
      url: compileScalar(r.create.map.url, 'create.map.url'),
    },
    squashMerge: { command: r.squashMerge.command },
  };
  if (r.pipelineStatus !== undefined) {
    assertPlaceholders(r.pipelineStatus.command, CR_ID_PLACEHOLDERS, 'pipelineStatus');
    routes.pipelineStatus = {
      command: r.pipelineStatus.command,
      status: compileScalar(r.pipelineStatus.statusPath, 'pipelineStatus.statusPath'),
      stateMap: r.pipelineStatus.stateMap,
    };
  }
  if (r.failingJobs !== undefined) {
    assertPlaceholders(r.failingJobs.command, CR_ID_PLACEHOLDERS, 'failingJobs');
    const template = r.failingJobs.map[0]!;
    routes.failingJobs = {
      command: r.failingJobs.command,
      itemsPath: compileItemsPath(r.failingJobs.itemsPath, 'failingJobs.itemsPath'),
      item: {
        name: compileScalar(template.name, 'failingJobs.map.name'),
        logExcerpt: compileScalar(template.logExcerpt, 'failingJobs.map.logExcerpt'),
      },
    };
  }
  return { authEnv: raw.authEnv, routes };
}
