/**
 * Pure diff helper for the neutral-prompt mirror invariant.
 *
 * The repo ships two parallel "neutral fallback command prompt" trees that must
 * stay byte-for-byte identical: `.pi/prompts` (the canonical source of truth —
 * `DEFAULT_TEMPLATES_DIR` in src/pack-generator.ts) and `.claude/commands` (a
 * hand-maintained mirror). `pack:check` guards the GENERATED `.adw/prompts`
 * tree, but nothing guarded the `.pi/prompts` ↔ `.claude/commands` mirror beyond
 * a single unit test; this module is the shared, gate-enforced guard (driven by
 * `tools/mirror-check.ts` / `npm run mirror:check`).
 *
 * It reads the filesystem but never touches `process`, prints, or exits — so it
 * is unit-testable over temp dirs and reusable by both the CLI and the test
 * suite, mirroring the pure/shell split of pack-generator.ts.
 */

import { readFileSync, readdirSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

import { resolveRepoPath } from '../src/config.js';
import { AdwError } from '../src/errors.js';

/** Canonical mirror pairs: source of truth → mirror that must match it byte-for-byte. */
export const MIRROR_PAIRS: ReadonlyArray<{ source: string; mirror: string }> = [
  { source: '.pi/prompts', mirror: '.claude/commands' },
];

export interface MirrorDiff {
  /** Source-relative paths present in source but missing from the mirror. */
  missing: string[];
  /** Source-relative paths present in the mirror but not in source. */
  extra: string[];
  /** Source-relative paths present in both but whose bytes differ. */
  drifted: string[];
}

export interface MirrorResult extends MirrorDiff {
  ok: boolean;
  source: string; // resolved absolute source dir
  mirror: string; // resolved absolute mirror dir
}

/**
 * Sorted, source-relative file paths under `dir` (recursive, regular files only,
 * POSIX-separated). Throws on any non-file/non-dir entry (symlink, socket, fifo)
 * so a symlinked prompt can never masquerade as identical content.
 */
export function listFilesRecursive(dir: string): string[] {
  const out: string[] = [];
  const walk = (abs: string): void => {
    for (const entry of readdirSync(abs, { withFileTypes: true })) {
      const child = join(abs, entry.name);
      if (entry.isDirectory()) {
        walk(child);
      } else if (entry.isFile()) {
        out.push(relative(dir, child).split(sep).join('/'));
      } else {
        // Symlinks / sockets / fifos are not expected in a prompt tree; fail loud.
        throw new AdwError(`unexpected non-regular entry in mirror tree: ${child}`);
      }
    }
  };
  walk(dir);
  return out.sort();
}

/**
 * Compare two trees byte-for-byte over the union of their files. Pure (fs read
 * only): never writes, prints, or exits. `ok` is true only when the file sets
 * match exactly and every common file is byte-identical.
 */
export function diffMirror(sourceDir: string, mirrorDir: string): MirrorResult {
  const source = resolveRepoPath(sourceDir);
  const mirror = resolveRepoPath(mirrorDir);
  const srcFiles = new Set(listFilesRecursive(source));
  const mirFiles = new Set(listFilesRecursive(mirror));

  const missing = [...srcFiles].filter((f) => !mirFiles.has(f)).sort();
  const extra = [...mirFiles].filter((f) => !srcFiles.has(f)).sort();
  const drifted = [...srcFiles]
    .filter((f) => mirFiles.has(f))
    .filter((f) => readFileSync(join(source, f)).compare(readFileSync(join(mirror, f))) !== 0)
    .sort();

  return {
    ok: missing.length === 0 && extra.length === 0 && drifted.length === 0,
    missing,
    extra,
    drifted,
    source,
    mirror,
  };
}
