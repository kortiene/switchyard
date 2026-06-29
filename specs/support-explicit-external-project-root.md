# Spec: Support an explicit external project root for ADW project packs

- **Issue:** #56 — `feat: Support explicit external project root for ADW project packs`
- **Labels:** `issue_class:feat`, `backlog`, `area:runtime`, `area:cli`
- **Type:** runtime + CLI feature (new flag/env + path-resolution seam); additive, backward-compatible
- **Primary new surface:** `--project-root <dir>` flag and `ADW_PROJECT_ROOT` env, a `projectRoot()` resolution seam, and a package-root prompt/schema fallback
- **Files touched (production):** `src/common.ts`, `src/config.ts`, `src/state.ts`, `src/run-phase.ts`, `src/exec.ts`, `src/phases.ts`, `src/schema-registry.ts`, `src/cli.ts`, `src/orchestrator.ts`, `src/env-vars.ts`, `src/index.ts`
- **Verification:** focused vitest per file, then `npm run verify` (from `adw_sdlc/`)

---

## 1. Context & current state (read this first)

### 1.1 One constant conflates two distinct roots

The package has a single notion of "root", computed once at import time from the
module's own file URL:

```ts
// src/common.ts:24
export const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
```

`src/` and `dist/` both sit directly under `adw_sdlc/`, so two levels up is the
**repository root** — for this standalone port that is
`/Users/sekou/TAC/pi-gh-issue` (the Switchyard monorepo root, the parent of the
`adw_sdlc/` package). Confirmed on disk: `.adw/`, `.pi/`, `.claude/`, `agents/`
all live at that repo root, not inside `adw_sdlc/`.

`REPO_ROOT` is doing **two jobs at once** that the issue asks us to separate:

| Job | Today keyed off | Should be keyed off |
|---|---|---|
| **Kernel / code location** (bundled neutral prompt templates `.pi/prompts`, generated `.adw/prompts`, default schemas, the parity fixtures) | `REPO_ROOT` | **package root** (stays `REPO_ROOT`) |
| **Project pack** (`.adw/config.json`, prompts/schema overrides, `agents/` state, agent `cwd`/worktree, git ops, local gate) | `REPO_ROOT` (+ `process.cwd()` for git/gate) | **project root** (new, configurable) |

Today these coincide because the project being orchestrated *is* the package's
own repo. The feature splits them so Switchyard can orchestrate an **external**
target repo without being copied into it.

### 1.2 Exact call sites that resolve against the root

Every project-scoped path flows through one of these (verified by grep):

- **Config file location** — `src/config.ts:18`
  `export const ADW_CONFIG_PATH = join(REPO_ROOT, '.adw', 'config.json')` (a
  `const` evaluated at import), consumed by `loadAdwConfig()` (`config.ts:401`)
  and cached by `getAdwConfig()` (`config.ts:418`).
- **Config-relative paths** (prompt roots, schema roots/overrides) —
  `src/config.ts:435` `resolveRepoPath(p) = resolve(REPO_ROOT, p)`, called by:
  - `src/phases.ts:212,217` `templatePath()` (prompt template resolution),
  - `src/schema-registry.ts:93,100,141` `overridePath()` (schema override/custom
    schema resolution),
  - `src/pack-generator.ts:259,260,325` (build-time prompt generator — **must
    stay package-root**, it generates the package's own prompts; see §4.2).
- **State / workspace dir** — `src/state.ts:31-33`
  `agentsDir() = agentsDirOverride ?? join(REPO_ROOT, 'agents')`. Every run's
  `state.json`, per-phase `prompt.txt`/`transcript.log`, and `metrics.json` are
  written under `agents/{adw_id}/` here (`state.ts:140-154,212-216`).
- **Agent `cwd` / worktree** — `src/run-phase.ts:114`
  `cwd: options.cwd ?? REPO_ROOT`. The orchestrator never passes `cwd`, so the
  agent currently edits at `REPO_ROOT`. This `cwd` flows into every runner SDK
  (`runner-claude.ts:215`, `runner-codex.ts:193`, `runner-opencode.ts:330/344`,
  `runner-pi.ts:280`) as the working directory the agent edits.
- **git / gh / gate commands** — these do **not** use `REPO_ROOT`; they run in
  `process.cwd()`. `src/git.ts` (`createOrCheckoutBranch`, `commitAll`, `push`,
  `pullRebase`) and `src/exec.ts` (`workingTreeDirty`, `detectRepo`,
  `issueState`, gate `runCmd`) all funnel through `capture()` (`exec.ts:101`),
  which calls `spawnSync` with **no `cwd`**, inheriting the parent process cwd.

### 1.3 Why the observed dry-run printed `(none configured)`

Observed command (run from inside `adw_sdlc/`):

```bash
npm run issue -- 5 --repo kortiene/iroh-room --runner claude --dry-run --allow-dirty
# → [dry-run] test gate: (none configured)
```

Flow: `printPlan()` (`orchestrator.ts:1039`) prints
`opts.testCmd || '(none configured)'`. `opts.testCmd` is resolved in
`resolveOptions()` (`orchestrator.ts:142`) as
`options.testCmd ?? getAdwConfig().commands.defaultTestCommand`. With no
`--test-cmd` / `ADW_TEST_CMD` passed, it falls back to `getAdwConfig()` — which
loads **Switchyard's own** `.adw/config.json` (`defaultTestCommand: ""`), not
iroh-room's. Hence `(none configured)`.

Once `getAdwConfig()` is made to load the *target* repo's
`/Users/sekou/TAC/iroh-room/.adw/config.json` (whose
`commands.defaultTestCommand` is `scripts/verify.sh`), the same dry-run will
print `scripts/verify.sh` with **no other code change** on that line. The whole
feature reduces to: *teach the root-resolution seam where the project lives, and
call it before the config is first read.*

### 1.4 Two subtleties discovered (these shape the design — do not skip)

1. **`validatePhaseChain()` runs during `--dry-run`, before `printPlan()`**
   (`orchestrator.ts:1113` then `1115`). It calls `templatePath()` +
   `existsSync` for every phase (`phases.ts:151-158`). If the project root is an
   external repo that does **not** ship `.adw/prompts/*.md`, dry-run will
   **throw `phase "…" is missing its prompt template`** *before* it ever prints
   the test gate — so AC #1/#2 cannot pass on a prompt-less target unless the
   kernel's bundled prompts are used as a fallback. ⇒ The package-root
   prompt/schema fallback (§4.2 / Decision **D2**) is **required**, not optional.

2. **The gate must keep running in `process.cwd()` when no project root is set.**
   In-repo, operators `cd adw_sdlc && ADW_TEST_CMD="npm run verify"`; the gate
   needs cwd = `adw_sdlc/` (where `package.json` lives). But `REPO_ROOT` is the
   *parent* of `adw_sdlc/` (no `package.json`). So we must **not** redirect
   git/gate cwd to `REPO_ROOT` in the default case — only when an explicit
   project root is given. This means the *asset root* (config/prompts/state,
   default `REPO_ROOT`) and the *command cwd* (git/gate/gh, default
   "inherit `process.cwd()`") have **different defaults**; see Decision **D3**.

### 1.5 Established idioms this spec mirrors

- **Process-global override + setter**, exactly like
  `state.ts` `agentsDirOverride` / `setAgentsDir()` (`state.ts:28-38`) and
  `config.ts` `testOverride` / `setAdwConfigForTests()` (`config.ts:414-432`).
  Tests already set and reset these in teardown; the new `projectRoot` seam
  follows the same lifecycle.
- **Flag + env fallback with precedence**, like `--repo`/`REPO`
  (`cli.ts:314`) and `--test-cmd`/`ADW_TEST_CMD` (`cli.ts:313`).
- **Canonical `ADW_*` env via the alias table** (`env-vars.ts` `ENV_ALIASES`).
- **Fail-closed validation with actionable `AdwError`**, like `validateAdwId`
  (`state.ts:49`) and the provider host/credential guards.

---

## 2. Goal & acceptance criteria

### Goal
Let Switchyard orchestrate an external target repository — loading **that
repo's** `.adw/config.json`, prompts/schemas, `agents/` state, and editing/
git-operating/gating **in that repo's worktree** — selected by an explicit
`--project-root` flag (or `ADW_PROJECT_ROOT`). The Switchyard package root
remains only the kernel/code location and the source of bundled-default
prompts/schemas. When the flag/env is omitted, behavior is byte-for-byte what it
is today.

### Acceptance criteria (from the issue)
- [ ] **AC1** Dry-run from the Switchyard package can target an external repo
  and load that repo's `.adw/config.json`.
- [ ] **AC2** Dry-run prints the target repo's configured test gate.
- [ ] **AC3** Agent phase `cwd` is the target project root, not the Switchyard
  repo.
- [ ] **AC4** `agents/{adw_id}` state is written under the target project root
  unless explicitly overridden.
- [ ] **AC5** Existing in-repo behavior remains backward-compatible when
  `--project-root` is omitted.
- [ ] **AC6** Path traversal and non-directory project roots fail closed with
  actionable errors.

### Additional acceptance criteria (this spec)
- [ ] **AC7** git/gh and the local gate command execute in the target project
  root when `--project-root` is set, and in `process.cwd()` when it is omitted
  (the in-repo `npm run verify` gate keeps working).
- [ ] **AC8** Prompt templates and JSON schemas resolve from the project root
  first and **fall back to the package root** (bundled kernel defaults), so a
  target that customizes only `.adw/config.json` still runs.
- [ ] **AC9** `ADW_PROJECT_ROOT` is treated as control-plane env: it is withheld
  from runner children (covered by the `ADW_` deny prefix, asserted by a test)
  and never added to the env allowlist.
- [ ] **AC10** `--project-root` precedence is flag > `ADW_PROJECT_ROOT` env >
  default (package root); a relative value resolves against `process.cwd()`.
- [ ] **AC11** `npm run verify` stays green (typecheck, `lint:env`, `pack:check`,
  `mirror:check`, coverage, build).

---

## 3. Design overview

Introduce **one resolution seam** with two accessors, defaulting to today's
behavior:

- `projectRoot(): string` → the **asset root** (config, prompts, schemas,
  `agents/`, agent `cwd`). Default = `REPO_ROOT` (package root) ⇒ unchanged when
  unset.
- `commandCwd(): string | undefined` → the **command cwd** for subprocesses
  (git/gh/gate). Returns the explicit project root **only when set**, else
  `undefined` (spawn inherits `process.cwd()`) ⇒ unchanged when unset.

Both read a single module-global `projectRootOverride` (default `null`), set once
per run by `setProjectRoot()` from the CLI/orchestrator. The package root stays
`REPO_ROOT` and is the second tier of prompt/schema resolution.

```
                         --project-root / ADW_PROJECT_ROOT  (validated, absolute)
                                          │
                                setProjectRoot(dir|null)
                                          │
                        projectRootOverride : string | null
                          /                               \
        projectRoot() = override ?? REPO_ROOT       commandCwd() = override ?? undefined
        (config, prompts*, schemas*, agents/,        (git / gh / gate via capture();
         agent cwd)                                    undefined ⇒ inherit process.cwd())
                         *prompts/schemas: project root → package-root fallback
```

---

## 4. Key design decisions

### D1 — Where the root seam lives, and the config-cache interaction

Put `REPO_ROOT`, `projectRootOverride`, `projectRoot()`, `commandCwd()`, and
`setProjectRoot()` in **`src/common.ts`**. Rationale: `common.ts` already owns
`REPO_ROOT` and has **no internal imports** except `errors.js`, so every module
(`config`, `state`, `run-phase`, `exec`, `phases`) can import the accessors with
**zero new import cycles**. (Putting it in `config.ts` would force `state.ts` to
take a new dependency on `config.ts`; `common.ts` avoids that.)

`setProjectRoot()` is kept **pure** (validate + set the override). It does **not**
reach into `config.ts` to clear the cache (that would create a `common → config`
cycle). Instead, make the config cache **root-aware** (Decision **D4**) so it
self-heals. This keeps the dependency graph acyclic and removes any "set the root
before/after the cache warms" ordering trap.

### D2 — Prompt/schema resolution falls back to the package root (REQUIRED)

Because `validatePhaseChain()` runs during dry-run (§1.4.1), and because an
external target generally won't ship the full `.adw/prompts` tree, prompt and
schema resolution must try the **project root first, then the package root**:

- `templatePath()` (`phases.ts:207`) builds candidate roots
  `[runnerRoot, defaultRoot]` resolved under **`projectRoot()`**, then the same
  two resolved under **`REPO_ROOT`**, returning the first existing file; the
  not-found error path points at the project-root default for a clear message.
- `overridePath()` (`schema-registry.ts:90`) tries the project-root
  `schemas.root`/override path, then the package-root equivalent.

This is additive: in-repo, `projectRoot() === REPO_ROOT`, so both tiers are the
same directory and behavior is unchanged. It is what makes "orchestrate a target
that only customizes `config.json`" actually work end-to-end (not just dry-run).

`pack-generator.ts` keeps resolving against the **package root** only (it
generates the package's own prompts; see §4.2). Achieve this by having it call a
package-root resolver, not the project-root `resolveRepoPath` (see step 5.7).

### D3 — Asset root vs command cwd have different defaults

`projectRoot()` defaults to `REPO_ROOT`; `commandCwd()` defaults to `undefined`
(inherit `process.cwd()`). This split is the crux of backward-compat (§1.4.2):
in-repo, the gate must run in `adw_sdlc/` (= `process.cwd()`), not in `REPO_ROOT`
(the parent, which has no `package.json`). git treats any subdir of a repo the
same, and gh uses `--repo`, so inheriting `process.cwd()` is exactly today's
behavior. Only when an explicit project root is provided do git/gh/gate move to
it (AC7).

### D4 — Make `getAdwConfig()` cache root-aware

`getAdwConfig()` (`config.ts:418`) memoizes into `cachedConfig`. Add a
`cachedRoot` alongside it; on read, reload when `cachedRoot !== projectRoot()`
(and the `setAdwConfigForTests` override still short-circuits first). This makes
the cache correct regardless of when `setProjectRoot()` is called, and means
tests that flip the project root see the right config without manual cache
busting. `ADW_CONFIG_PATH` (a `const`) becomes a function `adwConfigPath()`
returning `join(projectRoot(), '.adw', 'config.json')`; keep `ADW_CONFIG_PATH`
exported as a deprecated alias if any test imports it (grep: only `index.ts`
re-exports it — see step 5.2 for the compatibility shim).

### D5 — `ADW_PROJECT_ROOT` joins the `ENV_ALIASES` table

Add `projectRoot: { canonical: 'ADW_PROJECT_ROOT', legacy: 'MX_AGENT_PROJECT_ROOT' }`
to `ENV_ALIASES` (`env-vars.ts:10`). Reading via `readEnvAlias` gives the
established precedence/conflict-detection behavior for free and keeps every
control-plane env read centralized (consistent with the `drift-guard-adw-env-naming`
guard). `ADW_PROJECT_ROOT` is automatically covered by `ENV_DENY_PREFIXES`
(`env.ts:39` includes `'ADW_'`), so it is withheld from runner children with no
allowlist change (AC9). The legacy `MX_AGENT_PROJECT_ROOT` twin never existed in
mx-agent; it is included only for table uniformity — see **O-1** for the
alternative (canonical-only read) if a never-used legacy alias is unwanted.

### D6 — Validation: fail closed, canonicalize, don't weaken the secret boundary

`setProjectRoot(raw)` (or a helper `resolveProjectRoot(raw)` it calls):

1. `resolve(process.cwd(), raw)` → absolute (relative inputs resolve against the
   invocation cwd, matching shell intuition; AC10).
2. `realpathSync()` → canonicalizes `..` and symlinks and **throws on a
   non-existent path**; wrap as `AdwError('project root does not exist: <p>')`.
3. `statSync(canonical).isDirectory()` else
   `AdwError('project root is not a directory: <p>')`.
4. Store the canonical absolute path.

Interpretation of "path traversal fails closed" (AC6): a project root that does
not resolve to a real **directory** is rejected loudly with the offending path —
no silent fallback to the package root, no partial run. There is no enclosing
boundary to escape (the operator chooses the root), so canonicalization +
existence + directory checks are the closure. The env allowlist stays
kernel-owned and unchanged; the project root only moves the worktree/config/cwd,
never what reaches a runner child (Security Notes in the issue; §7).

---

## 5. Step-by-step implementation

### 5.1 `src/common.ts` — the root seam
- Keep `REPO_ROOT` (rename its doc-comment to "**package root** — the kernel/code
  location and the source of bundled-default prompts/schemas").
- Add module state + accessors:
  ```ts
  import { realpathSync, statSync } from 'node:fs';
  // ...
  let projectRootOverride: string | null = null;

  /** The asset root for config/prompts/schemas/agents/agent-cwd (default: package root). */
  export function projectRoot(): string {
    return projectRootOverride ?? REPO_ROOT;
  }

  /** Subprocess cwd for git/gh/gate: the explicit project root, else undefined (inherit process.cwd()). */
  export function commandCwd(): string | undefined {
    return projectRootOverride ?? undefined;
  }

  /** Validate + canonicalize a project root, or throw AdwError (fail closed). */
  export function resolveProjectRoot(raw: string): string {
    const abs = resolve(process.cwd(), raw);
    let real: string;
    try {
      real = realpathSync(abs);
    } catch {
      throw new AdwError(`project root does not exist: ${abs}`);
    }
    if (!statSync(real).isDirectory()) {
      throw new AdwError(`project root is not a directory: ${real}`);
    }
    return real;
  }

  /** Set (or clear, with null) the explicit project root. Pure: cache invalidation is root-aware (config.ts). */
  export function setProjectRoot(dir: string | null): void {
    projectRootOverride = dir === null ? null : resolveProjectRoot(dir);
  }
  ```
- `resolve` is already imported (`common.ts:13`); add `realpathSync, statSync`
  and `AdwError` is already imported (`common.ts:16`).

### 5.2 `src/config.ts` — project-root config + root-aware cache
- Replace the `ADW_CONFIG_PATH` const with a function and keep a compat export:
  ```ts
  import { projectRoot } from './common.js';
  export function adwConfigPath(): string {
    return join(projectRoot(), '.adw', 'config.json');
  }
  /** @deprecated use adwConfigPath(); kept for back-compat with existing importers. */
  export const ADW_CONFIG_PATH = adwConfigPath(); // evaluated at import = package root (today's value)
  ```
  (The `const` snapshot stays correct for the default case; all *runtime* reads
  must call `adwConfigPath()`.)
- `loadAdwConfig(path = adwConfigPath())`.
- `resolveRepoPath(p)` → `resolve(projectRoot(), p)` (was `resolve(REPO_ROOT, p)`).
  Add a sibling for the package tier used by the prompt/schema fallback and the
  generator:
  ```ts
  import { REPO_ROOT } from './common.js';
  export function resolvePackagePath(p: string): string {
    return resolve(REPO_ROOT, p);
  }
  ```
- Make the cache root-aware:
  ```ts
  let cachedConfig: AdwConfig | null = null;
  let cachedRoot: string | null = null;
  export function getAdwConfig(): AdwConfig {
    if (testOverride !== null) return testOverride;
    const root = projectRoot();
    if (cachedConfig === null || cachedRoot !== root) {
      cachedConfig = loadAdwConfig();
      cachedRoot = root;
    }
    return cachedConfig;
  }
  ```
  `setAdwConfigForTests(null)` should also reset `cachedRoot = null`.

### 5.3 `src/state.ts` — `agents/` under the project root
- `agentsDir()` → `agentsDirOverride ?? join(projectRoot(), 'agents')`
  (import `projectRoot` from `common.js`; drop the direct `REPO_ROOT` import if
  now unused). `setAgentsDir` remains the explicit override (satisfies AC4's
  "unless explicitly overridden").

### 5.4 `src/run-phase.ts` — agent cwd defaults to the project root
- Line 114: `cwd: options.cwd ?? projectRoot()` (import `projectRoot`; keep
  `REPO_ROOT` import only if still referenced — it will not be). When no project
  root is set, `projectRoot() === REPO_ROOT`, so this is identical to today
  (AC5); when set, the agent edits the target worktree (AC3).

### 5.5 `src/exec.ts` — subprocess cwd
- Extend `capture` to honor `commandCwd()` by default and accept an explicit
  override:
  ```ts
  import { commandCwd } from './common.js';
  export function capture(cmd, opts?: { env?: Record<string,string>; cwd?: string }): Captured {
    // ...
    const cwd = opts?.cwd ?? commandCwd();
    const spawnOpts: SpawnSyncOptions = { encoding: 'utf8' };
    if (opts?.env) spawnOpts.env = opts.env;
    if (cwd !== undefined) spawnOpts.cwd = cwd;
    const result = spawnSync(bin, args, spawnOpts);
    // ...
  }
  ```
  This routes git (`git.ts` via `capture`), gh queries, `workingTreeDirty`, and
  the orchestrator gate `runCmd` (`orchestrator.ts:224-227` → `capture`) to the
  project root when set, and inherits `process.cwd()` when not (AC7, D3). No
  change to declarative-provider callers that pass `{ env }` (they gain the same
  cwd, which is correct).

### 5.6 `src/phases.ts` + `src/schema-registry.ts` — package-root fallback (D2)
- `templatePath()` (`phases.ts:207`):
  ```ts
  import { resolveRepoPath, resolvePackagePath } from './config.js';
  export function templatePath(runner, name, config = getAdwConfig()): string {
    const roots = [config.prompts.runnerRoots[runner], config.prompts.defaultRoot]
      .filter((r): r is string => typeof r === 'string' && r.length > 0);
    const candidates = [
      ...roots.map((r) => join(resolveRepoPath(r), `${name}.md`)),     // project root
      ...roots.map((r) => join(resolvePackagePath(r), `${name}.md`)),  // package fallback
    ];
    for (const c of candidates) if (existsSync(c)) return c;
    return join(resolveRepoPath(config.prompts.defaultRoot), `${name}.md`); // for the not-found error
  }
  ```
- `overridePath()` (`schema-registry.ts:90`): after the project-root candidate
  misses, try `join(resolvePackagePath(root), `${phase}.json`)`; the explicit
  `overrides[phase]` path stays project-root-resolved (an explicit override is a
  deliberate project choice — do not silently fall back, keep its "not found"
  loud). Custom-phase schemas may also fall back to the package tier.

### 5.7 `src/pack-generator.ts` — pin to the package root
- Swap its three `resolveRepoPath(...)` calls (`259,260,325`) to
  `resolvePackagePath(...)`. The generator authors the package's own
  `.adw/prompts` from `.pi/prompts` + `pack.profile.json`; it must never follow a
  project-root override (a build-time tool, never run by the ADW runtime).

### 5.8 `src/env-vars.ts` — the env alias (D5)
- Add `projectRoot: { canonical: 'ADW_PROJECT_ROOT', legacy: 'MX_AGENT_PROJECT_ROOT' }`
  to `ENV_ALIASES`.

### 5.9 `src/cli.ts` — flag parsing
- Add `'--project-root'` to `VALUE_FLAGS` (`cli.ts:160`).
- In `parseCliArgs`:
  ```ts
  const projectRootArg = str('--project-root') ?? readEnvAlias(env, ENV_ALIASES.projectRoot);
  // ...in options:
  ...(projectRootArg !== undefined ? { projectRoot: projectRootArg } : {}),
  ```
- Add a help line to `CLI_USAGE` (`cli.ts:192`):
  `  --project-root <dir>     target repo root for config/prompts/state/worktree (env: ADW_PROJECT_ROOT)`.

### 5.10 `src/orchestrator.ts` — wire the option in, earliest
- Add `projectRoot?: string` to `RunOptions` (`orchestrator.ts:93`). It is only
  needed before `resolveOptions`, so read it from the **raw** `options` — do not
  thread it through `ResolvedOptions`.
- As the **first statement** of `run()` (before `const opts = resolveOptions(...)`
  at `orchestrator.ts:1092`):
  ```ts
  setProjectRoot(options.projectRoot ?? null);
  ```
  This guarantees `resolveOptions` (reads `getAdwConfig().commands.defaultTestCommand`)
  and `defaultDeps()` (`createProvidersFromConfig(getAdwConfig(), …)`) both see
  the target config (fixes AC1/AC2). The root-aware cache (D4) makes this correct
  even if `getAdwConfig()` warmed earlier.
- Add an observability line to `printPlan()` (`orchestrator.ts:1026`):
  `console.log(`[dry-run] project root: ${projectRoot()}`);` (testable; helps the
  AC1 demo).
- Consider resetting `setProjectRoot(null)` in a `finally` is **not** needed for
  the CLI one-shot process, but **is** needed for tests (handled in teardown,
  §6). Document that `run()` does not reset it (the process exits after one run);
  tests own the reset.

### 5.11 `src/index.ts` — exports
- Export `projectRoot`, `commandCwd`, `setProjectRoot`, `resolveProjectRoot` from
  `common.js`; `adwConfigPath`, `resolvePackagePath` from `config.js`. Keep
  existing exports (`REPO_ROOT`, `resolveRepoPath`, `ADW_CONFIG_PATH`,
  `agentsDir`, `setAgentsDir`) for back-compat.

---

## 6. Test plan

Mirror existing suites (`test/config.test.ts`, `test/state.test.ts`,
`test/run-phase.test.ts`, `test/cli.test.ts`, `test/orchestrator.test.ts`,
`test/env*.test.ts`). Use `mkdtempSync(os.tmpdir())` fixtures for external roots,
and **always reset `setProjectRoot(null)` + `setAdwConfigForTests(null)` +
`setAgentsDir(null)` in `afterEach`**.

1. **`resolveProjectRoot` / validation (AC6).**
   - existing dir → returns canonical absolute path;
   - non-existent path → `AdwError` "does not exist" with the path;
   - a regular file (not dir) → `AdwError` "is not a directory";
   - relative input resolves against `process.cwd()` (AC10);
   - a `..`-laden path to a real dir canonicalizes; to a missing dir fails.

2. **Config loads from the project root (AC1/AC2/D4).** Create a temp dir with
   `.adw/config.json` whose `commands.defaultTestCommand` = `"scripts/verify.sh"`.
   `setProjectRoot(tmp)`; assert `getAdwConfig().commands.defaultTestCommand ===
   'scripts/verify.sh'` and `adwConfigPath()` points inside `tmp`. Then
   `setProjectRoot(null)` and assert the cache reloaded to the package default
   (root-aware cache).

3. **`agentsDir()` follows the project root (AC4).** With `setProjectRoot(tmp)`,
   `agentsDir() === join(tmp, 'agents')` and a saved `AdwState` writes
   `state.json` under `tmp/agents/{id}/`. With `setAgentsDir(other)` set,
   `other` wins (explicit override). Unset → `join(REPO_ROOT, 'agents')` (AC5).

4. **Agent cwd (AC3/AC5).** In `run-phase`, with a mock runner capturing
   `req.cwd`: `setProjectRoot(tmp)` ⇒ `cwd === tmp`; unset ⇒ `cwd === REPO_ROOT`;
   explicit `options.cwd` still wins.

5. **Command cwd (AC7/D3).** Spy on `capture`/`spawnSync` (or use a tiny script
   that prints `pwd`): with `setProjectRoot(tmp)`, a git/gate `capture` runs with
   `cwd === tmp`; unset ⇒ `cwd` is `undefined` (inherits `process.cwd()`).
   Regression-guard the in-repo case: the gate is **not** redirected to
   `REPO_ROOT` when unset.

6. **Prompt/schema fallback (AC8/D2).** Temp project root with `.adw/config.json`
   but **no** `.adw/prompts`: `templatePath('claude','plan')` resolves to the
   **package** `.adw/prompts/plan.md`; with a project-local `.adw/prompts/plan.md`
   present, the project copy wins. `validatePhaseChain(DEFAULT_PHASES,'claude')`
   does **not** throw on a prompt-less target (the dry-run path). Schema override:
   project `.adw/schemas/tests.json` wins; absent ⇒ package fallback; an explicit
   `overrides[phase]` pointing at a missing file still throws loudly.

7. **CLI parsing (AC10).** `parseCliArgs(['5','--project-root','/x'])` →
   `options.projectRoot === '/x'`; `ADW_PROJECT_ROOT=/y` env with no flag →
   `'/y'`; flag overrides env; `--project-root` with no value → `AdwError`
   "requires a value"; help text includes the flag.

8. **End-to-end dry-run (AC1/AC2 — the issue's exact scenario).** Drive `run()`
   with `{ dryRun: true, projectRoot: tmp, repo: 'kortiene/iroh-room' }` (tmp has
   `.adw/config.json` with `defaultTestCommand: 'scripts/verify.sh'` and no
   prompts), capturing stdout via the injected seams. Assert the output contains
   `[dry-run] test gate: scripts/verify.sh` and `[dry-run] project root: <tmp>`,
   and that it returns `0` without throwing.

9. **Secret boundary (AC9).** Assert `'ADW_PROJECT_ROOT'` matches an
   `ENV_DENY_PREFIXES` entry and that `safeSubprocessEnv({ allowGhToken:false,
   runner:'claude', source:{ ADW_PROJECT_ROOT:'/x', … } })` does **not** contain
   `ADW_PROJECT_ROOT` (extend `test/env.test.ts`). Add `ADW_PROJECT_ROOT` to the
   live secret-boundary audit's denied-keys set if that fixture enumerates them.

10. **`readEnvAlias` projectRoot pair (D5).** Canonical wins; legacy warns;
    conflicting values throw (mirror `test/env-vars.test.ts`).

11. **No prompt-pack drift.** Confirm `pack:check`/`mirror:check` are unaffected
    (no `.adw/` or template content changes in this feature).

---

## 7. Security analysis (issue "Security Notes")

- **The env allowlist stays kernel-owned and unchanged.** `BASE_ENV_ALLOW`,
  `RUNNER_ENV_ALLOW`, and `ENV_DENY_PREFIXES` (`env.ts`) are untouched. The
  project root changes only the worktree/config root and subprocess cwd — never
  *what* env reaches a runner child. `ADW_PROJECT_ROOT` is itself withheld from
  children by the existing `ADW_` deny prefix (AC9).
- **Fail-closed root validation** (D6) rejects non-existent / non-directory /
  unresolvable roots with actionable `AdwError`s before any side effect (AC6).
- **Trust note (new attack surface — call out, don't silently expand).** Pointing
  the orchestrator at an *external* repo means that repo's `.adw/config.json` now
  influences this process: test/finalize gate commands, prompt/schema paths, and
  provider descriptors come from the target. This is the same trust the operator
  already extends by running the target's `scripts/verify.sh`, but it is broader
  than the in-repo case. Recommend a one-line README caution: *only target repos
  you trust to run commands on your machine.* Containment of config-relative
  paths (e.g. a malicious `prompts.defaultRoot: "../../etc"`) is **not** newly
  introduced by this change (today's `resolveRepoPath` already does no
  containment) — see **O-2** for an optional hardening.

---

## 8. Backward compatibility (AC5)

When `--project-root`/`ADW_PROJECT_ROOT` is omitted, `projectRootOverride`
stays `null`, so:
- `projectRoot() === REPO_ROOT` ⇒ config/prompts/schemas/agents/agent-cwd resolve
  exactly as today;
- `commandCwd() === undefined` ⇒ git/gh/gate inherit `process.cwd()` exactly as
  today (the in-repo `npm run verify` gate keeps working — the key subtlety of
  §1.4.2);
- the prompt/schema fallback tier equals the primary tier (same directory), so it
  is a no-op;
- `ADW_CONFIG_PATH` keeps its package-root snapshot value.

No state-schema change (`agents/{adw_id}/state.json` shape is untouched — only
*where* the directory lives changes), so cross-language compatibility is
preserved.

---

## 9. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Gate wrongly redirected to `REPO_ROOT` (no `package.json`) in the default case | `commandCwd()` returns `undefined` when unset (D3); test 5 regression-guards it. |
| Dry-run throws on a prompt-less target before printing the gate (§1.4.1) | Package-root prompt/schema fallback (D2/§5.6); test 6 + 8. |
| Stale config cache after a late `setProjectRoot` | Root-aware cache keyed on `projectRoot()` (D4); test 2. |
| New import cycle from the root seam | Seam lives in `common.ts` (no internal deps but `errors`); §D1. |
| `pack-generator` accidentally follows a project root | Pin it to `resolvePackagePath` (§5.7). |
| Tests leak global project-root state across cases | Mandatory `afterEach` reset of `setProjectRoot/​setAgentsDir/​setAdwConfigForTests` (§6). |
| Untrusted target config runs arbitrary gate commands | Documented trust note (§7); fail-closed validation; secret boundary intact. |
| `ADW_PROJECT_ROOT` leaking to a runner | Covered by `ADW_` deny prefix; asserted by test 9. |
| `realpathSync` rejects a valid root behind a broken symlink | Error message includes the resolved path; operator passes a canonical dir. |

---

## 10. Docs to update

- `adw_sdlc/README.md` — add `--project-root` to the flags table and the
  `ADW_PROJECT_ROOT` env note; one line under "Two layers: kernel + project pack"
  that the project pack can now live in an external repo; the §7 trust caution.
- `adw_sdlc/docs/UNIVERSAL.md` — document the kernel(package)-root vs
  project-root split and the prompt/schema fallback order.
- `adw_sdlc/HANDOVER.md` — record the new seam (`projectRoot()`/`commandCwd()`/
  `setProjectRoot()`), the config-cache root-awareness, and bump any test-count
  line the repo maintains.
- No `.adw/pack.profile.json` / `.adw/prompts` change ⇒ `pack:generate` not
  required (keeps `pack:check`/`mirror:check` green).

---

## 11. Out of scope

- A separate `--agents-dir` / `ADW_AGENTS_DIR` flag (the `setAgentsDir` override
  seam already satisfies AC4's "unless explicitly overridden"; a new flag can be
  a follow-up — **O-3**).
- Multi-root / monorepo sub-package targeting beyond a single project root.
- Copying or scaffolding `.adw/` into a target that lacks one (the fallback
  handles prompts/schemas; `config.json` absence already yields safe defaults via
  `loadAdwConfig`).
- Changing the env allowlist, secret boundary, or any runner adapter.
- Config-relative path **containment** hardening (optional — **O-2**).

---

## 12. Open questions

- **O-1.** Add a never-used `MX_AGENT_PROJECT_ROOT` legacy twin for `ENV_ALIASES`
  uniformity (recommended for consistency + `readEnvAlias` reuse), or read
  `ADW_PROJECT_ROOT` canonical-only (avoids inventing a deprecated alias)?
  Recommendation: the alias-table entry (D5); flagged here in case the maintainer
  prefers canonical-only.
- **O-2.** Should config-relative roots (`prompts.defaultRoot`, `schemas.root`,
  overrides) be **contained** within `projectRoot()` (reject `..` escapes) now
  that the config may come from a less-trusted external repo? Not a regression
  (today does no containment), but the external use case raises the stakes.
- **O-3.** Is an explicit `--agents-dir`/`ADW_AGENTS_DIR` wanted in this issue, or
  deferred? AC4 says "unless explicitly overridden"; the test-only `setAgentsDir`
  seam covers the literal AC, but operators may want a runtime override to keep
  run state outside the target worktree.
- **O-4.** Should `printPlan` also print the resolved config path and prompt
  source (project vs package fallback) for operator clarity, or is the
  `project root` line enough?
- **O-5.** *(Resolved.)* Confirmed on disk: `/Users/sekou/TAC/iroh-room/.adw`
  ships its **own full `.adw/prompts` tree** (classify, plan, implement, tests,
  resolve_failed_test, e2e_tests, review_phase, patch, document) and a
  `.adw/config.json` with `commands.defaultTestCommand: "scripts/verify.sh"`
  (matching AC2's expected output). So for *this* target D2's prompt fallback is
  belt-and-suspenders, **but**: (a) the target has **no** `.adw/schemas` dir, so
  D2's schema fallback to the package tier is load-bearing for any overridable/
  custom phase; and (b) D2 remains required in general for targets that customize
  only `config.json` — and §1.4.1 still bites during `--dry-run` for any
  prompt-less target. Keep D2.
```
