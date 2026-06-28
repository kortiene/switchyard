/**
 * Project-pack prompt generator.
 *
 * The neutral *template* prompts (`.pi/prompts/*.md`, mirrored byte-for-byte in
 * `.claude/commands/*.md` — enforced by `npm run mirror:check`, repaired by
 * `npm run mirror:sync`; see tools/mirror.ts) are the single source of truth.
 * A project's runtime
 * *pack* prompts (`.adw/prompts/*.md`, the `config.prompts.defaultRoot` this
 * repo ships) are GENERATED from those templates plus a project profile
 * (`.adw/pack.profile.json`) — so the pack is reproducible and the project's
 * identity/constraints live in data, not in hand-maintained forks of every
 * prompt.
 *
 * Two template primitives, chosen to NOT collide with the runtime Pi-style
 * `$`-substitution (`$1`, `$ARGUMENTS`, `${@:2}` — see common.ts substituteArgs)
 * that the generated pack still carries through to the agent:
 *
 *   1. `{{ var }}`  — inline variable substitution from `profile.vars`.
 *                     An undefined var is a hard error (never emit `{{…}}`).
 *   2. `<!-- adw:block NAME -->default…<!-- adw:endblock -->`
 *                   — a named block whose inner text is the NEUTRAL default;
 *                     `profile.blocks[NAME]`, when present, REPLACES the inner
 *                     text. `{{vars}}` inside either the default or the override
 *                     are substituted afterwards. A block lets a template stay
 *                     neutral while the profile injects project-specific context
 *                     (e.g. HealthTech's zero-knowledge constraints).
 *
 * Fail-closed by construction: malformed block markers, an unterminated block,
 * or any leftover `{{…}}` / `adw:block` marker after rendering throws AdwError,
 * so a half-substituted template can never reach a runner. Rendering is pure
 * (string → string) and deterministic; `generatePack`/`checkPack` add the fs
 * orchestration and a CI drift guard.
 */

import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { z } from 'zod';

import { resolveRepoPath } from './config.js';
import { AdwError } from './errors.js';

/** Default template source root (neutral prompts; `.claude/commands` mirrors it — `npm run mirror:check`). */
export const DEFAULT_TEMPLATES_DIR = '.pi/prompts';
/** Default generated-pack output root (this repo's `config.prompts.defaultRoot`). */
export const DEFAULT_PACK_DIR = '.adw/prompts';
/** Default profile path. */
export const DEFAULT_PROFILE_PATH = '.adw/pack.profile.json';

const ContextSectionSchema = z
  .object({
    heading: z.string().min(1),
    body: z.string().min(1),
  })
  .strict();

const ContextHeaderSchema = z
  .object({
    enabled: z.boolean().default(true),
    title: z.string().min(1).default('Project context'),
    /** Phase/template basenames to exclude, with or without `.md` (e.g. `classify`). */
    exclude: z.array(z.string().min(1)).default([]),
    sections: z.array(ContextSectionSchema).default([]),
  })
  .strict()
  .default({ enabled: true, title: 'Project context', exclude: [], sections: [] });

const MetapromptSchema = z
  .object({
    enabled: z.boolean().default(false),
    instructions: z.string().min(1).default(
      'Refine the deterministic prompt into a project-specific runtime prompt. Preserve every runtime argument token exactly ($1, $ARGUMENTS, ${@:2}, etc.). Preserve YAML frontmatter and all required output/artifact instructions. Do not add claims unsupported by the profile.',
    ),
    phaseGuidance: z.record(z.string(), z.string()).default({}),
  })
  .strict()
  .default({
    enabled: false,
    instructions:
      'Refine the deterministic prompt into a project-specific runtime prompt. Preserve every runtime argument token exactly ($1, $ARGUMENTS, ${@:2}, etc.). Preserve YAML frontmatter and all required output/artifact instructions. Do not add claims unsupported by the profile.',
    phaseGuidance: {},
  });

/**
 * A project profile: the data a pack is generated from. `vars` fill `{{ … }}`
 * placeholders; `blocks` override named `adw:block` defaults; `contextHeader`
 * injects profile-driven context after YAML frontmatter, with per-phase
 * exclusions; `metaprompt` configures the optional offline LLM refinement pass.
 */
export const PackProfileSchema = z
  .object({
    project: z
      .object({
        id: z.string().min(1),
        name: z.string().min(1),
      })
      .strict(),
    vars: z.record(z.string(), z.string()).default({}),
    blocks: z.record(z.string(), z.string()).default({}),
    contextHeader: ContextHeaderSchema,
    metaprompt: MetapromptSchema,
  })
  .strict();

export type PackProfile = z.infer<typeof PackProfileSchema>;

/** Match a well-formed block span; `[\s\S]*?` is non-greedy so adjacent blocks don't merge. */
const BLOCK_RE = /<!--\s*adw:block\s+([A-Za-z0-9_-]+)\s*-->([\s\S]*?)<!--\s*adw:endblock\s*-->/g;
/** Any block-ish marker, used to detect malformed/orphaned markers after resolution. */
const STRAY_MARKER_RE = /adw:(block|endblock)\b/;
/** Inline variable placeholder. */
const VAR_RE = /\{\{\s*([A-Za-z0-9_]+)\s*\}\}/g;
/** Any residual `{{` after substitution (empty name, malformed, or unknown shape). */
const RESIDUAL_VAR_RE = /\{\{/;

/**
 * Render one template's text into its pack form. Pure and deterministic:
 *   1. resolve every `adw:block` (override from `profile.blocks` else default),
 *   2. inject the project context header after YAML frontmatter when enabled,
 *   3. fail on any leftover/orphaned block marker,
 *   4. substitute `{{ var }}` from `profile.vars` (undefined ⇒ throw),
 *   5. fail on any residual `{{`.
 * `label` (a filename) is only used to make errors actionable.
 */
export function renderPackPrompt(text: string, profile: PackProfile, label = '<template>'): string {
  // 1. Resolve blocks. The override (or the default inner) may itself contain
  //    `{{vars}}`; those are handled by the global var pass in step 3.
  const resolved = text.replace(BLOCK_RE, (_whole, name: string, inner: string) => {
    const override = profile.blocks[name];
    return override !== undefined ? override : inner;
  });

  const withHeader = injectContextHeader(resolved, profile, label);

  // 2. A surviving marker means an unterminated block, a stray `adw:endblock`,
  //    or a marker smuggled in through an override value — fail closed.
  const strayMarker = STRAY_MARKER_RE.exec(withHeader);
  if (strayMarker) {
    throw new AdwError(
      `${label}: malformed or unterminated adw:block marker near "${strayMarker[0]}" ` +
        `(expected <!-- adw:block NAME -->…<!-- adw:endblock -->)`,
    );
  }

  // 3. Substitute vars; collect unknowns so the error lists them all at once.
  const missing = new Set<string>();
  const substituted = withHeader.replace(VAR_RE, (_whole, name: string) => {
    const value = profile.vars[name];
    if (value === undefined) {
      missing.add(name);
      return '';
    }
    return value;
  });
  if (missing.size > 0) {
    throw new AdwError(
      `${label}: undefined template var(s): ${[...missing].sort().join(', ')} ` +
        `(add to profile.vars)`,
    );
  }

  // 4. Any residual `{{` is a malformed placeholder (e.g. `{{ }}`, `{{1bad}}`).
  if (RESIDUAL_VAR_RE.test(substituted)) {
    throw new AdwError(`${label}: residual "{{" after substitution — malformed placeholder`);
  }

  return substituted;
}

/** Render the profile-driven header block for a phase, or `null` when excluded/empty. */
export function contextHeaderFor(profile: PackProfile, label: string): string | null {
  const header = profile.contextHeader;
  if (!header.enabled || header.sections.length === 0 || isExcluded(label, header.exclude)) {
    return null;
  }
  const lines: string[] = [
    `## ${header.title}`,
    '',
    '<!-- Generated project context. Edit .adw/pack.profile.json and run `npm run pack:generate`; do not hand-edit this block in .adw/prompts. -->',
  ];
  for (const section of header.sections) {
    lines.push('', `### ${section.heading}`, '', section.body.trim());
  }
  return `${lines.join('\n')}\n`;
}

/** Insert the project-context header after YAML frontmatter, preserving it byte-for-byte. */
export function injectContextHeader(text: string, profile: PackProfile, label: string): string {
  const header = contextHeaderFor(profile, label);
  if (header === null) {
    return text;
  }
  if (!text.startsWith('---\n')) {
    return `${header}\n${text}`;
  }
  const end = text.indexOf('\n---\n', 4);
  if (end === -1) {
    throw new AdwError(`${label}: malformed YAML frontmatter (missing closing ---)`);
  }
  const after = end + '\n---\n'.length;
  return `${text.slice(0, after)}${header}\n${text.slice(after)}`;
}

/** Load + validate a pack profile from JSON. */
export function loadPackProfile(path: string): PackProfile {
  let raw: string;
  try {
    raw = readFileSync(path, 'utf8');
  } catch (err) {
    throw new AdwError(`could not read pack profile: ${path}`, { cause: err });
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw) as unknown;
  } catch (err) {
    throw new AdwError(`pack profile is not valid JSON: ${path}`, { cause: err });
  }
  const result = PackProfileSchema.safeParse(parsed);
  if (!result.success) {
    throw new AdwError(`invalid pack profile (${path}): ${result.error.issues.map((i) => `${i.path.join('.') || '<root>'} ${i.message}`).join('; ')}`);
  }
  return result.data;
}

/** The `.md` template basenames under `dir`, sorted for deterministic output. */
export function listTemplates(dir: string): string[] {
  let entries: string[];
  try {
    entries = readdirSync(dir);
  } catch (err) {
    throw new AdwError(`could not read templates dir: ${dir}`, { cause: err });
  }
  return entries.filter((name) => name.endsWith('.md')).sort();
}

/** One file's render result, used by both generate and check. */
export interface RenderedFile {
  /** Template basename (e.g. `plan.md`). */
  name: string;
  /** Absolute source template path. */
  templatePath: string;
  /** Absolute destination pack path. */
  outPath: string;
  /** Rendered pack text (always ends with exactly one trailing newline). */
  content: string;
}

export interface GenerateOptions {
  /** Template source dir (default `.pi/prompts`, repo-relative unless absolute). */
  templatesDir?: string;
  /** Pack output dir (default `.adw/prompts`). */
  outDir?: string;
  /** The validated profile. */
  profile: PackProfile;
}

/** Render every template against the profile, without touching disk. */
export function renderPack(options: GenerateOptions): RenderedFile[] {
  const templatesDir = resolveRepoPath(options.templatesDir ?? DEFAULT_TEMPLATES_DIR);
  const outDir = resolveRepoPath(options.outDir ?? DEFAULT_PACK_DIR);
  const names = listTemplates(templatesDir);
  if (names.length === 0) {
    throw new AdwError(`no .md templates found in ${templatesDir}`);
  }
  return names.map((name) => {
    const templatePath = join(templatesDir, name);
    const text = readFileSync(templatePath, 'utf8');
    const rendered = renderPackPrompt(text, options.profile, name);
    return {
      name,
      templatePath,
      outPath: join(outDir, name),
      content: ensureTrailingNewline(rendered),
    };
  });
}

/** Build the offline LLM metaprompt for one already-rendered deterministic prompt. */
export function buildMetaprompt(file: RenderedFile, profile: PackProfile): string {
  const phase = file.name.replace(/\.md$/, '');
  const phaseGuidance = profile.metaprompt.phaseGuidance[phase] ?? profile.metaprompt.phaseGuidance[file.name] ?? '';
  const profileSummary = {
    project: profile.project,
    vars: profile.vars,
    contextHeader: profile.contextHeader,
  };
  return [
    '# Prompt-pack refinement task',
    '',
    profile.metaprompt.instructions,
    '',
    `Phase/template: ${file.name}`,
    phaseGuidance ? `\nPhase-specific guidance:\n${phaseGuidance}` : '',
    '',
    'Project profile data:',
    '```json',
    JSON.stringify(profileSummary, null, 2),
    '```',
    '',
    'Deterministic prompt draft to refine:',
    '```md',
    file.content.trimEnd(),
    '```',
    '',
    'Return only the final Markdown prompt body, including the original YAML frontmatter if present.',
  ]
    .filter((part) => part !== '')
    .join('\n');
}

export interface GenerateResult {
  /** Files whose on-disk content changed (or were created). */
  written: string[];
  /** Files already up to date. */
  unchanged: string[];
}

/**
 * Generate the pack to disk. Idempotent: a file is rewritten only when its
 * rendered bytes differ, so re-running is a no-op and `written` is empty.
 * `dryRun` reports what WOULD change without writing.
 */
export function generatePack(options: GenerateOptions & { dryRun?: boolean }): GenerateResult {
  const rendered = renderPack(options);
  const outDir = resolveRepoPath(options.outDir ?? DEFAULT_PACK_DIR);
  if (options.dryRun !== true) {
    mkdirSync(outDir, { recursive: true });
  }
  const written: string[] = [];
  const unchanged: string[] = [];
  for (const file of rendered) {
    if (readIfExists(file.outPath) === file.content) {
      unchanged.push(file.name);
      continue;
    }
    written.push(file.name);
    if (options.dryRun !== true) {
      writeFileSync(file.outPath, file.content, 'utf8');
    }
  }
  return { written, unchanged };
}

export interface CheckResult {
  ok: boolean;
  /** Pack files whose on-disk content differs from a fresh render. */
  drifted: string[];
  /** Templates with no corresponding pack file on disk. */
  missing: string[];
}

/**
 * Verify the committed pack matches a fresh render (CI drift guard). Never
 * writes. `ok` is true only when every template has an up-to-date pack file.
 */
export function checkPack(options: GenerateOptions): CheckResult {
  const rendered = renderPack(options);
  const drifted: string[] = [];
  const missing: string[] = [];
  for (const file of rendered) {
    const current = readIfExists(file.outPath);
    if (current === null) {
      missing.push(file.name);
    } else if (current !== file.content) {
      drifted.push(file.name);
    }
  }
  return { ok: drifted.length === 0 && missing.length === 0, drifted, missing };
}

function ensureTrailingNewline(text: string): string {
  return text.endsWith('\n') ? text : `${text}\n`;
}

function isExcluded(label: string, excluded: readonly string[]): boolean {
  const stem = label.replace(/\.md$/, '');
  return excluded.some((entry) => entry === label || entry.replace(/\.md$/, '') === stem);
}

function readIfExists(path: string): string | null {
  try {
    return readFileSync(path, 'utf8');
  } catch {
    return null;
  }
}
