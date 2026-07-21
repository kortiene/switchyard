/**
 * Shared helpers ported from adw/common.py: prompt-template rendering (the
 * Pi-style positional substitutions both template trees use) and the
 * fence/prose-tolerant JSON parser behind the fenced-JSON output contract.
 *
 * Project-pack prompt templates (for this standalone repo, .adw/prompts/) and
 * the neutral fallback command roots (.pi/prompts/ and .claude/commands/) share
 * the same Pi-style positional substitution contract — this module must keep
 * rendering them byte-for-byte the way adw/common.py does.
 */

import { readFileSync, realpathSync, statSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { AdwError } from './errors.js';

/**
 * Package root — the kernel/code location and the source of bundled-default
 * prompts/schemas. Both src/ and dist/ sit directly under adw_sdlc/, so two
 * levels up is the package's repository root from either layout — the directory
 * holding the shipped .adw/config.json, .adw/prompts, .claude/commands,
 * .pi/prompts, adw/state.schema.json, and agents/.
 *
 * This is ONLY the kernel location. The *project* being orchestrated (its
 * config/prompts/state/worktree) is the project root — see projectRoot(),
 * which defaults to REPO_ROOT but can point at an external target repo.
 */
export const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');

/**
 * The explicit project (asset/worktree) root for this run, or null to use the
 * package root. Set once per run by setProjectRoot() from the CLI/orchestrator.
 * A process-global override + setter, mirroring state.ts agentsDirOverride and
 * config.ts testOverride; tests set it and reset to null in teardown.
 */
let projectRootOverride: string | null = null;

/**
 * The asset root for this run: config (.adw/config.json), prompts/schemas
 * (project tier, with a package-root fallback), agents/ state, and the agent's
 * editing cwd. Defaults to the package root, so behavior is unchanged when no
 * explicit project root is set.
 */
export function projectRoot(): string {
  return projectRootOverride ?? REPO_ROOT;
}

/**
 * Subprocess cwd for git/gh/gate commands: the explicit project root when set,
 * else undefined so spawn inherits process.cwd(). The asset root and the
 * command cwd have DIFFERENT defaults on purpose: in-repo, the gate must run in
 * adw_sdlc/ (= process.cwd(), where package.json lives), not in REPO_ROOT (its
 * parent). Only an explicit project root redirects subprocess cwd.
 */
export function commandCwd(): string | undefined {
  return projectRootOverride ?? undefined;
}

/**
 * Validate + canonicalize a raw project-root value, or throw AdwError (fail
 * closed). Relative inputs resolve against the invocation cwd (shell
 * intuition); realpathSync canonicalizes `..`/symlinks and throws on a missing
 * path; a non-directory is rejected loudly. There is no enclosing boundary to
 * escape — the operator chooses the root — so existence + directory checks are
 * the closure (the env allowlist stays kernel-owned and unchanged).
 */
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

/**
 * Set (or clear, with null) the explicit project root. Pure: it only validates
 * and stores the override. Cache invalidation is root-aware in config.ts
 * (getAdwConfig reloads when projectRoot() changes), so this never reaches into
 * config.ts and no import cycle is introduced.
 */
export function setProjectRoot(dir: string | null): void {
  projectRootOverride = dir === null ? null : resolveProjectRoot(dir);
}

/** Remove YAML frontmatter from a prompt template body if present. */
export function stripFrontmatter(text: string): string {
  if (!text.startsWith('---\n')) {
    return text;
  }
  const end = text.indexOf('\n---\n', 4);
  if (end === -1) {
    return text;
  }
  return text.slice(end + '\n---\n'.length);
}

function sliceArgs(args: readonly string[], start: string, length?: string): string {
  const index = Number.parseInt(start, 10) - 1;
  if (index < 0) {
    throw new AdwError('argument slices are 1-indexed');
  }
  const selected =
    length === undefined ? args.slice(index) : args.slice(index, index + Number.parseInt(length, 10));
  return selected.join(' ');
}

/**
 * Apply Pi-style positional substitution to prompt-template text: `$1`, `$2`,
 * `$@`, `$ARGUMENTS`, `${@:N}`, and `${@:N:L}` (adw/common.py:70-90). The
 * single substitution engine shared by every renderer in this package.
 */
export function substituteArgs(text: string, args: readonly string[]): string {
  const allArgs = args.join(' ');
  let out = text.replaceAll('$ARGUMENTS', allArgs).replaceAll('$@', allArgs);
  out = out.replace(/\$\{@:(\d+)(?::(\d+))?\}/g, (_m, start: string, length?: string) =>
    sliceArgs(args, start, length),
  );
  out = out.replace(/\$(\d+)/g, (_m, num: string) => {
    const index = Number.parseInt(num, 10) - 1;
    return index >= 0 && index < args.length ? (args[index] ?? '') : '';
  });
  return out;
}

/**
 * Render a prompt template selected by filesystem path, stripping YAML
 * frontmatter first (adw/common.py render_prompt_file).
 */
export function renderPromptFile(path: string, args: readonly string[]): string {
  let text: string;
  try {
    text = readFileSync(path, 'utf8');
  } catch (err) {
    throw new AdwError(`prompt template not found: ${path}`, { cause: err });
  }
  return substituteArgs(stripFrontmatter(text), args);
}

const JSON_FENCE_RE = /```json\s*\n([\s\S]*?)\n```/gi;
const FENCE_RE = /```(?:[\w-]+)?\s*\n([\s\S]*?)\n```/g;

/**
 * Return balanced top-level `{...}`/`[...]` spans in source order.
 *
 * Agent prose can legitimately start with Markdown such as `[test]` before a
 * later JSON object. A first-bracket/last-bracket slice mistakes that tag for a
 * JSON array and discards the usable object. Track nesting and JSON strings so
 * each candidate can instead be parsed independently.
 */
function bracedJsonCandidates(text: string): string[] {
  const candidates: string[] = [];
  const stack: string[] = [];
  let start = -1;
  let inString = false;
  let escaped = false;

  for (let i = 0; i < text.length; i += 1) {
    const char = text[i]!;
    if (start === -1) {
      if (char === '{' || char === '[') {
        start = i;
        stack.push(char === '{' ? '}' : ']');
      }
      continue;
    }

    if (inString) {
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === '"') inString = false;
      continue;
    }
    if (char === '"') {
      inString = true;
      continue;
    }
    if (char === '{' || char === '[') {
      stack.push(char === '{' ? '}' : ']');
      continue;
    }
    if (char !== '}' && char !== ']') {
      continue;
    }
    if (stack.at(-1) !== char) {
      // Malformed prose candidate. Reset and keep scanning for a later JSON
      // value rather than letting one stray bracket consume the whole reply.
      start = -1;
      stack.length = 0;
      inString = false;
      escaped = false;
      continue;
    }
    stack.pop();
    if (stack.length === 0) {
      candidates.push(text.slice(start, i + 1));
      start = -1;
    }
  }
  return candidates;
}

function topLevelTypeError(result: unknown, expect?: 'object' | 'array'): AdwError | null {
  if (expect === 'object' && (typeof result !== 'object' || result === null || Array.isArray(result))) {
    return new AdwError(`expected a JSON object, got ${Array.isArray(result) ? 'array' : typeof result}`);
  }
  if (expect === 'array' && !Array.isArray(result)) {
    return new AdwError(`expected a JSON array, got ${typeof result}`);
  }
  return null;
}

/**
 * Parse JSON that may be wrapped in a Markdown code fence or surrounding
 * prose (adw/common.py parse_json). Prefers the LAST explicit ```json fence
 * (agents emit the contract block last); then the last generic fenced block;
 * then the first parseable balanced object/array span. Raises AdwError on
 * failure; `expect` constrains the parsed top-level type.
 */
export function parseJson(text: string | null | undefined, expect?: 'object' | 'array'): unknown {
  if (text === null || text === undefined) {
    throw new AdwError('no JSON to parse: empty agent output');
  }

  const jsonMatches = [...text.matchAll(JSON_FENCE_RE)];
  const genericMatches = jsonMatches.length > 0 ? [] : [...text.matchAll(FENCE_RE)];
  const matches = jsonMatches.length > 0 ? jsonMatches : genericMatches;
  const last = matches.length > 0 ? matches[matches.length - 1] : undefined;
  const candidate = last?.[1] !== undefined ? last[1].trim() : text.trim();
  const nested = bracedJsonCandidates(candidate);
  const attempts = [candidate, ...nested.filter((value) => value !== candidate)];
  let parseError: unknown;
  let typeError: AdwError | null = null;

  for (const attempt of attempts) {
    let result: unknown;
    try {
      result = JSON.parse(attempt);
    } catch (err) {
      parseError ??= err;
      continue;
    }
    const mismatch = topLevelTypeError(result, expect);
    if (mismatch !== null) {
      typeError ??= mismatch;
      continue;
    }
    return result;
  }

  if (typeError !== null) {
    throw typeError;
  }
  const snippet = candidate.slice(0, 200);
  throw new AdwError(
    `could not parse JSON from agent output: ${parseError instanceof Error ? parseError.message : String(parseError)} (saw: ${JSON.stringify(snippet)})`,
    { cause: parseError },
  );
}

/**
 * Split a gate command string into argv, honoring single/double quotes (the
 * minimal slice of shlex.split the finalize/test gates need; no escapes).
 */
export function shellSplit(command: string): string[] {
  const out: string[] = [];
  const re = /'([^']*)'|"([^"]*)"|(\S+)/g;
  for (const match of command.matchAll(re)) {
    out.push(match[1] ?? match[2] ?? match[3] ?? '');
  }
  return out;
}
