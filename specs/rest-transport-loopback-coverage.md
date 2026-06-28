# Spec: real `restTransportViaNode` loopback coverage (the HTTP path has zero tests)

- **Work item:** GitHub issue #26 — `Add real restTransportViaNode loopback coverage (the HTTP path has zero tests)`
- **Labels / class:** `issue_class:test`, `area:providers`, `backlog`
- **Source:** `adw_sdlc/docs/DESIGN-declarative-providers.md §12.2`; `adw_sdlc/src/providers-rest-cli.ts (restTransportViaNode)`; `adw_sdlc/test/providers.test.ts (fake transport only)`
- **Status:** specification only. **Do not implement as part of this phase.**

---

## 1. Goal

Give the **real** `restTransportViaNode` HTTP transport its first automated coverage. Today
every `rest` provider test injects a *fake* transport (`adw_sdlc/test/providers.test.ts:211`,
`:248`, `:268`, `:309`, `:457`, `:496`, `:699`, …), so the actually-shipped HTTP path —
`spawnSync` the kernel helper, marshal the request over stdin, read the token by name from the
child's scoped env, run `fetch()` under an `AbortSignal.timeout`, JSON round-trip the reply —
is exercised by **nothing**. `adw_sdlc/docs/DESIGN-declarative-providers.md:330-331` nonetheless
claims this path "is verified via a two-process loopback roundtrip", and `adw_sdlc/HANDOVER.md:717`
/ `:776` repeat a "live two-process loopback roundtrip" claim. Those describe a *one-time manual*
check performed during development; there is no committed, reproducible test.

This work adds a real loopback round-trip test against `restTransportViaNode`, covers the timeout
and host-allowlist re-assert behaviors, and reconciles the over-stated docs so the "verified"
claim becomes true and reproducible from a clean clone.

This is a **test + docs** item (`issue_class:test`). **No production behavior changes.**

---

## 2. Background — verify before writing

### 2.1 What is under test (`adw_sdlc/src/providers-rest-cli.ts`)

`restTransportViaNode` (`:199-214`) is the default `RestTransport` (`:171`) and an exported,
injectable seam:

```ts
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
```

The child script `REST_FETCH_SCRIPT` (`:179-196`) is kernel-owned `node -e` code that:

- reads the request JSON from **stdin** (fd `0`); invalid JSON ⇒ `{"status":0,"body":"","error":"invalid request"}`;
- sets `Accept: application/json`; reads the token via `process.env[req.authEnv]` (**by name, never argv**) and, when present, sets `req.authHeader` to `authScheme ? scheme + ' ' + token : token`;
- builds `init = { method, headers, signal: AbortSignal.timeout(req.timeoutMs) }`; when `req.body` is set, adds `content-type: application/json` and `JSON.stringify(body)`;
- `await fetch(req.url, init)` → writes `{ status, body }`; on throw → `{ status: 0, body: '', error }`.

Behaviors with **zero** current coverage: the `spawnSync` round-trip itself, stdin marshaling,
header assembly (Authorization-by-name, Accept, content-type), `AbortSignal.timeout`, `res.text()`
body round-trip, the parent's parse of child stdout, and the parent error branches (`:206`, `:211`).

### 2.2 Where the host re-assert lives (NOT in the transport)

The transport performs **no** host/https check — it `fetch`es whatever `req.url` it is handed. The
host-allowlist + https re-assertion is one layer up, in `makeRestRequester`'s `send`
(`adw_sdlc/src/providers-rest-cli.ts:268-269`):

```ts
const send = (method, url, body?) => {
  assertAllowedHost(url, base.allowedHosts);   // <-- the "host re-assert" (defense in depth)
  return transport({ ...req }, scopedEnv());
};
```

`assertAllowedHost` (`adw_sdlc/src/provider-descriptor.ts:406-419`) enforces **https-only** + an
**exact `host[:port]`** allowlist; `isAllowedHost` (`:427-434`) is its non-throwing predicate used
by pagination. Both already have direct unit coverage in
`adw_sdlc/test/provider-descriptor.test.ts:263-275` (https accepted, `http://` rejected,
off-allowlist rejected, unparseable ⇒ false). The off-allowlist *config* path is also covered in
`providers.test.ts:422-428` / `:525-531`, and the pagination off-allowlist stop in `:777-810`.

Consequence for this issue: the "host-allowlist re-assert" of acceptance bullet 2 is *logic that is
already unit-tested*; what is missing is proof that this guard actually **fronts the real
transport** — i.e. that `restTransportViaNode` never egresses to a non-https / off-allowlist target.
This spec adds that proof rather than re-testing the pure function (§5.4).

### 2.3 The load-bearing constraint: `spawnSync` blocks the event loop

`restTransportViaNode` uses `spawnSync`, which **does not return until the child process closes** —
it blocks the calling thread's libuv event loop for the whole HTTP exchange. Therefore a loopback
`http.Server` hosted in the **same process/event loop** as the test would be frozen during the call:
the spawned helper's `fetch` would open a TCP connection that the (blocked) server never accepts, and
the request would hang until the child's own `AbortSignal.timeout` fired. A naive single-process
loopback **deadlocks**.

This is almost certainly why the path was only ever "manually" verified. The fix is to host the
loopback server **off the test's main thread** so its event loop keeps running while `spawnSync`
blocks the main thread — a genuinely separate process (`child_process.fork`) or a `worker_threads`
worker. This spec recommends a forked Node child (§4 decision D1).

### 2.4 Test environment facts (verified)

- vitest runs in the default **node** environment (`adw_sdlc/vitest.config.ts` sets no `environment`),
  so `node:http`, `node:child_process`, `worker_threads`, global `fetch`, and `AbortSignal.timeout`
  are all available. Node is v22 (`@types/node ^22`).
- Package is ESM (`"type": "module"`); a helper run by Node directly (fork/worker target) must be
  ESM and use `import`, not `require`.
- `restTransportViaNode`, `RestRequest`, `RestResponse`, `RestTransport` are exported from
  `adw_sdlc/src/providers-rest-cli.ts` (and re-exported from `src/index.ts:183`). The existing test
  already imports `RestRequest`/`RestTransport` from `../src/providers-rest-cli.js`
  (`providers.test.ts:23-25`); the new test adds `restTransportViaNode`.
- `withScopedEnv` (`adw_sdlc/test/helpers.ts:11`) sets/restores `process.env` keys around a sync
  callback — used to assert the scoped-credential boundary. `safeSubprocessEnv`
  (`adw_sdlc/src/env.ts:111`) builds the production-shaped scoped env (BASE allowlist + one
  `extraAllow` credential, `GH_TOKEN` withheld).

---

## 3. Scope

**In scope**

- A committed test that round-trips a real request through `restTransportViaNode` against a loopback
  HTTP server (acceptance bullet 1).
- Timeout coverage and host-allowlist-re-assert coverage tying the guard to the real transport
  (acceptance bullet 2).
- Reconcile/repair the `§12.2` "verified via a two-process loopback roundtrip" claim and the matching
  `HANDOVER.md` lines (acceptance bullet 3).

**Out of scope**

- Any change to `restTransportViaNode`, `REST_FETCH_SCRIPT`, the requester, the descriptor, or the
  env boundary. This is a test/docs item; production code is read-only.
- An end-to-end round-trip through the **full provider → requester(https) → real transport** stack.
  That requires TLS termination on the loopback (the requester is https-only) and is deliberately
  **not** pursued — see §4 D2 and §10 O1. Acceptance bullet 1 explicitly says "loopback **http**-server
  … through `restTransportViaNode`", i.e. the transport called directly.
- TLS / proxy / redirect behavior of `fetch`.

---

## 4. Key decisions

**D1 — Host the loopback server in a forked Node child (separate process), not in-process.**
Required by §2.3 (`spawnSync` blocks the main thread). A `child_process.fork` of a tiny ESM helper
gives a fully independent event loop **and** process (the most faithful reading of "two-process
loopback": the forked server + the spawned `node -e` helper), with built-in IPC to hand back the
ephemeral port and the captured request. *Alternative:* a `worker_threads.Worker` (also off-thread,
lighter) — acceptable, but a forked process matches the documented framing and isolates the server's
sockets from the vitest worker. *Rejected:* in-process `http.Server` (deadlocks, §2.3).

**D2 — Drive the transport directly over plain HTTP; do not attempt an https full-stack round-trip.**
`restTransportViaNode(req, env)` is exported precisely as a seam and does no scheme check, so a
`http://127.0.0.1:<port>` URL exercises the entire spawn/stdin/fetch/JSON path. Going through the
full provider would hit the requester's https-only `assertAllowedHost`, forcing TLS on the loopback;
worse, a self-signed cert would be rejected by the child unless `NODE_TLS_REJECT_UNAUTHORIZED` were
forwarded — which the production scoped env (correctly) does not allow. So the full-stack https
variant cannot use the real production env and adds a vendored key (secret-scanner surface) for no
extra coverage of the transport. Keep the round-trip at the transport seam (matches the acceptance
wording), and cover the guard separately (D3).

**D3 — Cover "host re-assert" as *gating of the real transport*, leaning on existing unit tests for
the guard logic.** `assertAllowedHost`/`isAllowedHost` are already unit-tested
(`provider-descriptor.test.ts:263-275`). The new, missing assertion is that the guard **prevents the
real transport from egressing**: a loopback server is started, a provider/descriptor is pointed at it
over plain `http://`, the https guard rejects it (at construction), and the loopback server records
**zero** connections. This proves the re-assert fronts `restTransportViaNode`, which the pure-function
unit tests do not. (The send-time re-assert for a *primary* route is structurally unreachable with a
bad host — `baseUrl` is allowlist-validated at parse and placeholders are percent-encoded — so a
dedicated send-time failure test would need a new test-only export; see §10 O2.)

**D4 — Put the new tests in a dedicated file, `adw_sdlc/test/providers-rest-transport.test.ts`.**
The existing `providers.test.ts` is synchronous-by-design (mirroring `helpers.ts`'s note that
`withScopedEnv` must wrap sync callbacks). The loopback tests need `async` setup/teardown and a
forked-process lifecycle; isolating them keeps `providers.test.ts` sync and the server teardown
contained. *Alternative:* append to `providers.test.ts` (acceptable but mixes sync/async lifecycles).

**D5 — No new dependency.** Use only `node:http`, `node:child_process`, `node:events` (`once`), and
the existing `safeSubprocessEnv`/`withScopedEnv`. Consistent with the repo's no-new-dep posture for
the whole declarative-provider line (`DESIGN-declarative-providers.md:312-315`).

---

## 5. Implementation steps

### 5.1 Loopback server helper — `adw_sdlc/test/helpers/loopback-server.mjs` (new)

A tiny ESM module run as a **forked child**. It hosts an `http.Server` on `127.0.0.1:0`, captures
each request (method, url, headers, raw body), responds per a `mode`, and speaks a small IPC protocol
back to the parent. Plain JS (no TS transform; Node runs it directly).

```js
// test/helpers/loopback-server.mjs
import { createServer } from 'node:http';

const mode = process.env.LOOPBACK_MODE ?? 'echo';   // 'echo' | 'status404' | 'hang'
const captured = [];
const sockets = new Set();

const server = createServer((req, res) => {
  let body = '';
  req.on('data', (c) => (body += c));
  req.on('end', () => {
    captured.push({ method: req.method, url: req.url, headers: req.headers, body });
    if (mode === 'hang') return;                     // never respond ⇒ client AbortSignal fires
    if (mode === 'status404') {
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end('{"message":"nope"}');
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ ok: true, echoBody: body }));
  });
});

server.on('connection', (s) => {                     // track for forced teardown (hang mode)
  sockets.add(s);
  s.on('close', () => sockets.delete(s));
});

server.listen(0, '127.0.0.1', () => {
  process.send?.({ type: 'listening', port: server.address().port });
});

process.on('message', (m) => {
  if (m?.type === 'requests') process.send?.({ type: 'requests', items: captured });
  if (m?.type === 'close') {
    for (const s of sockets) s.destroy();            // unstick a 'hang'-held socket
    server.close(() => process.exit(0));
  }
});
```

### 5.2 Parent-side wrapper (top of the test file, or a small `test/helpers/loopback.ts`)

```ts
import { fork, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';

interface CapturedRequest { method: string; url: string; headers: Record<string, string | string[] | undefined>; body: string; }

interface Loopback {
  origin: string;                         // http://127.0.0.1:<port>
  port: number;
  requests(): Promise<CapturedRequest[]>; // queried after the (sync) transport call returns
  close(): Promise<void>;
}

async function startLoopback(mode: 'echo' | 'status404' | 'hang' = 'echo'): Promise<Loopback> {
  const child: ChildProcess = fork(new URL('./helpers/loopback-server.mjs', import.meta.url), [], {
    env: { ...process.env, LOOPBACK_MODE: mode },   // helper is a plain server; no secret boundary concern
    stdio: ['ignore', 'inherit', 'inherit', 'ipc'],
  });
  const [msg] = (await once(child, 'message')) as [{ type: string; port: number }];
  if (msg.type !== 'listening') throw new Error(`unexpected first message: ${msg.type}`);
  const port = msg.port;
  return {
    origin: `http://127.0.0.1:${port}`,
    port,
    requests: async () => {
      child.send({ type: 'requests' });
      const [r] = (await once(child, 'message')) as [{ type: string; items: CapturedRequest[] }];
      return r.items;
    },
    close: async () => {
      child.send({ type: 'close' });
      await once(child, 'exit');
    },
  };
}
```

Notes:
- `restTransportViaNode` is **synchronous**: call it inside the test (blocking), then `await
  lb.requests()` *after* it returns — the captured request is fetched over IPC once the main-thread
  event loop resumes.
- Always `await lb.close()` in a `finally` (or `afterEach`) so no forked process / held socket leaks.
- Forking is cheap and per-test here; if startup cost matters, a single shared 'echo' server reused
  across the happy-path cases is acceptable (capture array would then need clearing — keep per-test
  for simplicity).

### 5.3 Round-trip + transport-mechanics tests (acceptance bullet 1)

In `adw_sdlc/test/providers-rest-transport.test.ts`:

1. **GET round-trip with auth-by-name.** Start `echo`. Build a production-shaped scoped env and call
   the real transport:

   ```ts
   const lb = await startLoopback('echo');
   let res: RestResponse;
   withScopedEnv({ FORGE_TOKEN: 'tok-123', GH_TOKEN: 'gh-secret' }, () => {
     const env = safeSubprocessEnv({ allowGhToken: false, extraAllow: ['FORGE_TOKEN'] });
     expect(env['GH_TOKEN']).toBeUndefined();          // production scoping: GitHub authority withheld
     res = restTransportViaNode(
       { method: 'GET', url: `${lb.origin}/issues/42`, authEnv: 'FORGE_TOKEN',
         authHeader: 'Authorization', authScheme: 'Bearer', timeoutMs: 5000 },
       env,
     );
   });
   const [req] = await lb.requests();
   expect(res!.status).toBe(200);
   expect(JSON.parse(res!.body)).toMatchObject({ ok: true });
   expect(req.method).toBe('GET');
   expect(req.url).toBe('/issues/42');
   expect(req.headers['authorization']).toBe('Bearer tok-123');  // token read by NAME inside the child
   expect(req.headers['accept']).toBe('application/json');
   await lb.close();
   ```

   This single test exercises spawnSync, stdin marshaling, env-token-by-name, header assembly,
   `fetch`, `res.text()`, and the parent's JSON parse — the whole previously-untested path — and
   confirms the scoped env (no `GH_TOKEN`) end to end.

2. **POST with JSON body + content-type.** `echo`; `method: 'POST'`, `body: { state_event: 'Done' }`,
   `timeoutMs: 5000`. Assert the server saw `content-type: application/json`, `JSON.parse(req.body)`
   deep-equals `{ state_event: 'Done' }`, and the response round-trips (`res.status === 200`,
   `JSON.parse(res.body).echoBody` equals the sent JSON string). Covers the body branch of
   `REST_FETCH_SCRIPT:188`.

3. **No token ⇒ no Authorization header.** `echo`; `env` omits the `authEnv` key (or `authEnv` names
   an unset var). Assert `req.headers['authorization']` is `undefined` (covers the `if (token)`
   branch, `REST_FETCH_SCRIPT:186`).

4. **Non-2xx passthrough.** `status404`; assert `res.status === 404`, `res.body === '{"message":"nope"}'`,
   `res.error` is undefined (the transport faithfully relays non-2xx; `restOk` rejection is an
   upstream concern, already covered with fakes).

### 5.4 Timeout + host-re-assert tests (acceptance bullet 2)

5. **Timeout via `AbortSignal.timeout`.** `hang` (server captures but never responds); call the
   transport with a small `timeoutMs` (e.g. `250`). Assert `res.status === 0`, `res.body === ''`, and
   `res.error` matches `/abort|timeout/i` (the child's `fetch` rejects on abort →
   `REST_FETCH_SCRIPT:193-195`). Ensure `lb.close()` destroys the held socket (helper does this on
   `{type:'close'}`). Keep the timeout small so the test is fast.

6. **Transport-level error (connection refused).** Start `echo`, capture its `port`, `await
   lb.close()`, *then* call the transport against `http://127.0.0.1:<port>/x`. Assert `res.status === 0`
   and `res.error` is truthy (ECONNREFUSED surfaces through the child's catch → parent relays). A
   deterministic exercise of the error path without timing dependence.

7. **Host-allowlist re-assert gates the real transport (no egress).** Start `echo`. Point the
   descriptor at the loopback over **plain http** and confirm the https guard rejects it *before* any
   request reaches the server:

   ```ts
   const lb = await startLoopback('echo');
   expect(() =>
     parseRestWorkItemDescriptor({
       type: 'rest',
       baseUrl: lb.origin,                              // http://127.0.0.1:<port>  (non-https)
       allowedHosts: [`127.0.0.1:${lb.port}`],
       authEnv: 'FORGE_TOKEN',
       routes: { fetch: { path: '/x', map: { title: '$.t', body: '$.b', labels: '$.l[*]' } },
                 state: { path: '/x', map: { state: '$.s' } } },
     }),
   ).toThrow(/must be https/);
   expect(await lb.requests()).toHaveLength(0);          // the real transport never egressed
   await lb.close();
   ```

   Plus assert the standalone guard the requester calls behaves on loopback shapes (complements the
   existing `provider-descriptor.test.ts:263-275`): `assertAllowedHost('https://127.0.0.1:'+port+'/x',
   ['127.0.0.1:'+port])` does not throw, while the `http://` form throws `/must be https/` and an
   off-allowlist host throws `/not in allowedHosts/`. Together these cover "host re-assert" both as
   logic and as a gate in front of `restTransportViaNode`.

### 5.5 Reconcile the docs (acceptance bullet 3)

The claim that the real transport is "verified via a two-process loopback roundtrip" was, until this
work, **not** backed by a committed test. Make it true and point at the test:

1. `adw_sdlc/docs/DESIGN-declarative-providers.md §12 step 2b` (`:330-331`) — replace
   "Tests use an injected transport; the real transport is verified via a two-process loopback
   roundtrip." with wording that names the committed coverage, e.g.: "Tests inject a fake transport
   for the mapping/guard logic; the real `restTransportViaNode` HTTP path (spawn + stdin + auth-by-name
   + `AbortSignal` timeout + JSON round-trip) is covered by an automated loopback test,
   `test/providers-rest-transport.test.ts`."
2. `adw_sdlc/docs/DESIGN-declarative-providers.md §13` (`:354-356`) — update the `rest` testing-strategy
   bullet to describe what actually shipped: a forked loopback HTTP server round-tripping the real
   transport directly, plus the https/allowlist gate that blocks egress to a non-https/off-allowlist
   target. Drop the "(or a mocked helper)" hedge for the transport now that a real one exists.
3. `adw_sdlc/HANDOVER.md:716-720` (§8l) and `:775-779` (§8m) — the "live two-process loopback
   roundtrip" lines describe a manual, one-off check. **Do not rewrite the historical session log or
   its test counts**; instead add a short, additive note (a new dated subsection, e.g. §8q, or a
   one-line pointer appended to §8l) recording that issue #26 codified that round-trip as
   `test/providers-rest-transport.test.ts`, so the claim is now reproducible. (The `document` phase
   owns the exact HANDOVER prose; this spec only fixes the inaccurate "verified" claim per acceptance.)
4. No `.adw/pack.profile.json` change ⇒ **no** `npm run pack:generate` needed.

---

## 6. Owning modules / files touched

| File | Change |
| --- | --- |
| `adw_sdlc/test/providers-rest-transport.test.ts` (new) | the loopback tests (§5.3–5.4) |
| `adw_sdlc/test/helpers/loopback-server.mjs` (new) | forked loopback HTTP server (§5.1) |
| `adw_sdlc/docs/DESIGN-declarative-providers.md` | §12.2 + §13 reconcile (§5.5.1–2) |
| `adw_sdlc/HANDOVER.md` | additive note reconciling the manual-claim lines (§5.5.3) |

No production source files change. `src/providers-rest-cli.ts`, `src/provider-descriptor.ts`,
`src/env.ts` are read-only here.

---

## 7. Acceptance criteria mapping

| Issue acceptance bullet | Satisfied by |
| --- | --- |
| A loopback http-server round-trips a real rest request through `restTransportViaNode` | §5.1–5.3 (forked loopback + tests 1–4): real spawn/stdin/fetch/JSON round-trip, auth header by name, body + content-type, non-2xx passthrough |
| Timeout + host-allowlist re-assert covered | §5.4 test 5 (AbortSignal timeout ⇒ `status 0` + error) and test 7 (the https/allowlist guard gates the real transport — zero egress — plus the standalone guard on loopback shapes); test 6 adds the connection-error path |
| Reconcile or remove the §12.2 'verified' claim | §5.5 (rewrite §12.2 + §13 to name the committed test; additive HANDOVER reconcile of the matching manual-claim lines) |

---

## 8. Security & invariant preservation

- **No production change** ⇒ no behavior, secret-boundary, or schema impact. The transport, the
  kernel helper, the requester, and the env allowlist are untouched.
- **Secret boundary, exercised end-to-end (read-only):** test 1 builds the child env with
  `safeSubprocessEnv({ allowGhToken: false, extraAllow: ['FORGE_TOKEN'] })` and asserts `GH_TOKEN`
  is absent and that the helper reads the forge token **by name** from its scoped env — turning the
  boundary invariant (`env.ts`) into an executable assertion against the real spawn path. `lint:env`
  (`scripts/check-adw-sdlc-env.sh`) stays green: the test does not spread `process.env` into a runner
  (the loopback fork is a test fixture, not a runner child; the transport-under-test still uses the
  scoped env we pass it).
- **Loopback only:** the forked server binds `127.0.0.1:0` (ephemeral, localhost) — no external
  egress, no fixed port. `restTransportViaNode` is called only with `http://127.0.0.1:<port>` URLs the
  test controls.
- **No new dependency / no code loading** (D5).

---

## 9. Risks & mitigations

- **Same-process deadlock (the core trap).** If a future refactor moves the loopback server back into
  the test's own event loop, `spawnSync` will freeze it and the round-trip will hang to its timeout
  (§2.3). Mitigation: the helper is a **forked process**; a code comment in the test must state *why*
  (spawnSync blocks the main thread) so the off-thread server is not "simplified" away.
- **Leaked forked processes / sockets.** A failed assertion before `close()` could orphan the child.
  Mitigation: `try/finally` (or `afterEach`) always calls `lb.close()`; the helper destroys held
  sockets on close (needed for `hang` mode), and exits the process.
- **Timeout-test flakiness.** Too-small `timeoutMs` on a slow CI could abort before the request is
  even captured. Mitigation: `250ms` is comfortably above loopback connect latency yet fast; the test
  asserts only `status 0` + `/abort|timeout/i` (the captured request is incidental, not asserted in
  the hang case). The error path is *also* covered deterministically by the connection-refused test
  (test 6), which has no timing dependence.
- **Abort error message wording varies by Node.** Different Node versions phrase the abort as "The
  operation was aborted" / "This operation was aborted" / "timeout". Mitigation: assert the loose
  `/abort|timeout/i`, not an exact string.
- **fork vs worker portability.** `child_process.fork` of an ESM `.mjs` requires Node ≥ the repo's
  baseline (v22 here) — fine. If a future environment forbids `fork`, the worker_threads alternative
  (D1) is a drop-in.
- **vitest pool interaction.** vitest itself may run the test in a worker/fork; nesting our own fork
  inside is supported. No special vitest config is required (env is already `node`).

---

## 10. Open questions & assumptions

**Assumptions**
- A1. Acceptance bullet 1 means the transport called **directly** over a plain-http loopback (the
  bullet says "http-server … through `restTransportViaNode`"), not a full provider→requester(https)
  stack. The spec is built on this reading (D2).
- A2. A forked process is the intended "two-process" in the §12.2 claim (server fork + spawned helper).
  A `worker_threads` worker is an acceptable substitute if the maintainer prefers it.
- A3. HANDOVER's historical test counts and session logs should be preserved, not rewritten; only the
  inaccurate "verified" claim is reconciled, additively (A: matches the repo's append-only HANDOVER
  convention).

**Open questions**
- O1. Should a **gold-standard https full-stack** round-trip (provider → requester → real transport →
  TLS loopback) also be added? It would exercise the requester's send-time `assertAllowedHost` on the
  happy path, but needs a vendored self-signed cert and forwarding `NODE_TLS_REJECT_UNAUTHORIZED` to
  the child — which the production scoped env forbids by design. Recommendation: **no** (D2); revisit
  only if send-time re-assert coverage is independently required (O2).
- O2. Do we want a direct test of the **send-time** host re-assert (`makeRestRequester`'s per-call
  `assertAllowedHost`), distinct from the parse-time guard? It is structurally unreachable with a bad
  host via the public API (baseUrl is allowlist-validated; placeholders are percent-encoded; pagination
  pre-checks with `isAllowedHost`). Covering it would mean exporting `makeRestRequester` (a small
  test-only seam). Recommendation: skip unless the maintainer wants it; §5.4 test 7 already proves the
  guard fronts the real transport.
- O3. Per-test fork vs. one shared 'echo' server reused across the happy-path cases (faster, needs
  capture-array reset). Recommendation: per-test for clarity unless suite time regresses noticeably.

---

## 11. Verification

From `adw_sdlc/`:

1. Focused: `npx vitest run test/providers-rest-transport.test.ts` (new file).
2. Regression: `npx vitest run test/providers.test.ts test/provider-descriptor.test.ts` (unchanged
   suites stay green).
3. Full gate: `npm run verify` (typecheck, `lint:env`, prompt-pack drift check, tests, build, `dist/`
   cleanup).

If a check cannot be run, state exactly why and the command the maintainer should run.
