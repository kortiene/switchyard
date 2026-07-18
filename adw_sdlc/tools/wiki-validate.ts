#!/usr/bin/env node
/** Validate the repository's Open Knowledge Format (OKF) wiki bundle. */

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { TextDecoder } from 'node:util';

import { parseDocument } from 'yaml';

export interface WikiValidationIssue {
  path: string;
  message: string;
}

export interface WikiValidationResult {
  files: number;
  concepts: number;
  issues: WikiValidationIssue[];
}

interface FrontmatterResult {
  body: string;
  data: Record<string, unknown> | null;
  error?: string;
}

const TOOL_DIR = dirname(fileURLToPath(import.meta.url));
export const DEFAULT_WIKI_ROOT = resolve(TOOL_DIR, '../../wiki');
const RESERVED = new Set(['index.md', 'log.md']);

function within(parent: string, candidate: string): boolean {
  const rel = relative(parent, candidate);
  return rel === '' || (!rel.startsWith(`..${sep}`) && rel !== '..' && !isAbsolute(rel));
}

function markdownFiles(root: string): string[] {
  const files: string[] = [];
  const visit = (directory: string): void => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile() && entry.name.endsWith('.md')) files.push(path);
    }
  };
  visit(root);
  return files.sort();
}

function utf8(path: string): string {
  return new TextDecoder('utf-8', { fatal: true }).decode(readFileSync(path));
}

function frontmatter(text: string): FrontmatterResult {
  const lines = text.replace(/^\uFEFF/, '').split(/\r?\n/);
  if (lines[0] !== '---') return { body: text, data: null };
  const end = lines.indexOf('---', 1);
  if (end < 0) return { body: text, data: null, error: 'frontmatter has no closing --- delimiter' };

  const source = lines.slice(1, end).join('\n');
  try {
    const document = parseDocument(source, { uniqueKeys: true });
    if (document.errors.length > 0) {
      return {
        body: lines.slice(end + 1).join('\n'),
        data: null,
        error: document.errors[0]?.message ?? 'YAML parser rejected the document',
      };
    }
    const value: unknown = document.toJS();
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
      return { body: lines.slice(end + 1).join('\n'), data: null, error: 'frontmatter must be a YAML mapping' };
    }
    return { body: lines.slice(end + 1).join('\n'), data: value as Record<string, unknown> };
  } catch (error) {
    return {
      body: lines.slice(end + 1).join('\n'),
      data: null,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

function issue(
  issues: WikiValidationIssue[],
  wikiRoot: string,
  path: string,
  message: string,
): void {
  issues.push({ path: relative(wikiRoot, path).split(sep).join('/'), message });
}

function validateOptionalFields(
  issues: WikiValidationIssue[],
  wikiRoot: string,
  path: string,
  data: Record<string, unknown>,
): void {
  for (const key of ['title', 'description', 'resource'] as const) {
    const value = data[key];
    if (value !== undefined && (typeof value !== 'string' || value.trim() === '')) {
      issue(issues, wikiRoot, path, `${key} must be a non-empty string when present`);
    }
  }
  const tags = data['tags'];
  if (
    tags !== undefined &&
    (!Array.isArray(tags) || tags.some((tag) => typeof tag !== 'string' || tag.trim() === ''))
  ) {
    issue(issues, wikiRoot, path, 'tags must be a YAML list of non-empty strings when present');
  }
  const timestamp = data['timestamp'];
  if (
    timestamp !== undefined &&
    (typeof timestamp !== 'string' || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(timestamp))
  ) {
    issue(issues, wikiRoot, path, 'timestamp must be an ISO 8601 datetime string when present');
  }
}

function validateConcept(
  issues: WikiValidationIssue[],
  wikiRoot: string,
  path: string,
  text: string,
): void {
  const parsed = frontmatter(text);
  if (!text.replace(/^\uFEFF/, '').startsWith('---\n') && !text.replace(/^\uFEFF/, '').startsWith('---\r\n')) {
    issue(issues, wikiRoot, path, 'concept must begin with YAML frontmatter');
    return;
  }
  if (parsed.error !== undefined) {
    issue(issues, wikiRoot, path, `invalid YAML frontmatter: ${parsed.error}`);
    return;
  }
  if (parsed.data === null) {
    issue(issues, wikiRoot, path, 'concept frontmatter must be a YAML mapping');
    return;
  }
  const type = parsed.data['type'];
  if (typeof type !== 'string' || type.trim() === '') {
    issue(issues, wikiRoot, path, 'frontmatter must contain a non-empty type');
  }
  validateOptionalFields(issues, wikiRoot, path, parsed.data);
}

function validateIndex(
  issues: WikiValidationIssue[],
  wikiRoot: string,
  path: string,
  text: string,
): void {
  const isRoot = resolve(path) === resolve(wikiRoot, 'index.md');
  const parsed = frontmatter(text);
  if (parsed.error !== undefined) {
    issue(issues, wikiRoot, path, `invalid YAML frontmatter: ${parsed.error}`);
    return;
  }
  if (!isRoot && parsed.data !== null) {
    issue(issues, wikiRoot, path, 'frontmatter is permitted only in the bundle-root index.md');
  }
  if (isRoot && parsed.data !== null && parsed.data['okf_version'] !== '0.1') {
    issue(issues, wikiRoot, path, 'root index frontmatter must declare okf_version: "0.1"');
  }
  const body = parsed.body;
  if (!/^#\s+\S/m.test(body)) {
    issue(issues, wikiRoot, path, 'index must contain at least one level-one section heading');
  }
  if (!/^\s*[-*]\s+\[[^\]]+\]\([^)]+\)(?:\s+-\s+\S.*)?$/m.test(body)) {
    issue(issues, wikiRoot, path, 'index must contain at least one Markdown-link list entry');
  }
}

function realIsoDate(value: string): boolean {
  const parsed = new Date(`${value}T00:00:00Z`);
  return !Number.isNaN(parsed.valueOf()) && parsed.toISOString().slice(0, 10) === value;
}

function validateLog(
  issues: WikiValidationIssue[],
  wikiRoot: string,
  path: string,
  text: string,
): void {
  const parsed = frontmatter(text);
  if (parsed.data !== null || parsed.error !== undefined) {
    issue(issues, wikiRoot, path, 'log.md must not contain frontmatter');
  }
  if (!/^#\s+\S/m.test(text)) {
    issue(issues, wikiRoot, path, 'log must contain a level-one title');
  }
  const headings = [...text.matchAll(/^##\s+(.+)\s*$/gm)].map((match) => match[1]?.trim() ?? '');
  if (headings.length === 0) {
    issue(issues, wikiRoot, path, 'log must contain at least one YYYY-MM-DD update heading');
    return;
  }
  for (const heading of headings) {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(heading) || !realIsoDate(heading)) {
      issue(issues, wikiRoot, path, `invalid log date heading: ${heading}`);
    }
  }
  for (let index = 1; index < headings.length; index += 1) {
    if ((headings[index - 1] ?? '') < (headings[index] ?? '')) {
      issue(issues, wikiRoot, path, 'log date headings must be newest first');
      break;
    }
  }
  const groups = text.split(/^##\s+.+\s*$/gm).slice(1);
  if (groups.some((group) => !/^\s*[-*]\s+\S/m.test(group))) {
    issue(issues, wikiRoot, path, 'each log date group must contain at least one list entry');
  }
}

function withoutFencedCode(text: string): string {
  const visible: string[] = [];
  let fence: { marker: '`' | '~'; length: number } | null = null;
  for (const line of text.split(/\r?\n/)) {
    const candidate = line.match(/^\s*(`{3,}|~{3,})/);
    if (fence === null) {
      if (candidate?.[1] !== undefined) {
        fence = { marker: candidate[1][0] as '`' | '~', length: candidate[1].length };
        visible.push('');
      } else {
        visible.push(line);
      }
      continue;
    }
    const closing = new RegExp(`^\\s*${fence.marker}{${fence.length},}\\s*$`);
    if (closing.test(line)) fence = null;
    visible.push('');
  }
  return visible.join('\n').replace(/`[^`\n]*`/g, '');
}

function linkDestination(raw: string): string {
  const trimmed = raw.trim();
  if (trimmed.startsWith('<')) {
    const end = trimmed.indexOf('>');
    return end >= 0 ? trimmed.slice(1, end) : trimmed;
  }
  return trimmed.split(/\s+/, 1)[0] ?? '';
}

function validateLinks(
  issues: WikiValidationIssue[],
  wikiRoot: string,
  repoRoot: string,
  path: string,
  text: string,
): void {
  const body = withoutFencedCode(text);
  for (const match of body.matchAll(/!?\[[^\]]*\]\(([^)]+)\)/g)) {
    const raw = linkDestination(match[1] ?? '');
    if (
      raw === '' ||
      raw.startsWith('#') ||
      raw.startsWith('//') ||
      /^[a-z][a-z0-9+.-]*:/i.test(raw)
    ) {
      continue;
    }
    const pathOnly = raw.split(/[?#]/, 1)[0] ?? '';
    let decoded: string;
    try {
      decoded = decodeURIComponent(pathOnly);
    } catch {
      issue(issues, wikiRoot, path, `link has invalid percent encoding: ${raw}`);
      continue;
    }
    const target = decoded.startsWith('/')
      ? resolve(wikiRoot, decoded.slice(1))
      : resolve(dirname(path), decoded);
    if (!within(repoRoot, target)) {
      issue(issues, wikiRoot, path, `local link escapes the repository: ${raw}`);
      continue;
    }
    try {
      const stat = statSync(target);
      if (stat.isDirectory() && !existsSync(resolve(target, 'index.md'))) {
        issue(issues, wikiRoot, path, `linked directory has no index.md: ${raw}`);
      }
    } catch {
      issue(issues, wikiRoot, path, `broken local link: ${raw}`);
    }
  }
}

export function validateWiki(wikiRoot = DEFAULT_WIKI_ROOT): WikiValidationResult {
  const root = resolve(wikiRoot);
  const repoRoot = resolve(root, '..');
  const issues: WikiValidationIssue[] = [];
  let files: string[];
  try {
    files = markdownFiles(root);
  } catch (error) {
    return {
      files: 0,
      concepts: 0,
      issues: [{ path: '.', message: `cannot read wiki root ${root}: ${error instanceof Error ? error.message : String(error)}` }],
    };
  }

  let concepts = 0;
  for (const path of files) {
    let text: string;
    try {
      text = utf8(path);
    } catch (error) {
      issue(issues, root, path, `file is not valid UTF-8: ${error instanceof Error ? error.message : String(error)}`);
      continue;
    }
    const name = path.slice(path.lastIndexOf(sep) + 1);
    if (!RESERVED.has(name)) {
      concepts += 1;
      validateConcept(issues, root, path, text);
    } else if (name === 'index.md') {
      validateIndex(issues, root, path, text);
    } else {
      validateLog(issues, root, path, text);
    }
    validateLinks(issues, root, repoRoot, path, text);
  }

  if (!files.some((path) => resolve(path) === resolve(root, 'index.md'))) {
    issues.push({ path: 'index.md', message: 'bundle-root index.md is missing' });
  }
  issues.sort((a, b) => a.path.localeCompare(b.path) || a.message.localeCompare(b.message));
  return { files: files.length, concepts, issues };
}

const USAGE = `usage: wiki-validate [--check] [--root <directory>] [-h|--help]

Validate the Switchyard OKF wiki. --check is accepted for consistency and is the default.`;

export function main(argv: readonly string[]): number {
  let root = DEFAULT_WIKI_ROOT;
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--check') continue;
    if (arg === '-h' || arg === '--help') {
      process.stdout.write(`${USAGE}\n`);
      return 0;
    }
    if (arg === '--root') {
      const value = argv[index + 1];
      if (value === undefined) {
        process.stderr.write('error: --root requires a directory\n');
        return 2;
      }
      root = resolve(value);
      index += 1;
      continue;
    }
    process.stderr.write(`error: unknown flag: ${arg}\n`);
    return 2;
  }

  const result = validateWiki(root);
  if (result.issues.length > 0) {
    for (const validationIssue of result.issues) {
      process.stderr.write(`${validationIssue.path}: ${validationIssue.message}\n`);
    }
    process.stderr.write(
      `wiki validation failed: ${result.issues.length} issue(s) across ${result.files} Markdown file(s)\n`,
    );
    return 1;
  }
  process.stdout.write(`wiki is OKF-valid: ${result.concepts} concepts in ${result.files} Markdown files\n`);
  return 0;
}

if (process.argv[1] !== undefined && process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
