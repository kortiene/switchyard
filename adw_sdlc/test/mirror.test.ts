import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { AdwError } from '../src/errors.js';
import { diffMirror, listFilesRecursive, MIRROR_PAIRS } from '../tools/mirror.js';

function tmpDir(): string {
  return mkdtempSync(join(tmpdir(), 'adw-mirror-'));
}

function writeTree(root: string, files: Record<string, string>): void {
  for (const [rel, content] of Object.entries(files)) {
    const dest = join(root, rel);
    mkdirSync(join(dest, '..'), { recursive: true });
    writeFileSync(dest, content, 'utf8');
  }
}

describe('MIRROR_PAIRS', () => {
  it('declares exactly one pair: .pi/prompts -> .claude/commands', () => {
    expect(MIRROR_PAIRS).toHaveLength(1);
    const pair = MIRROR_PAIRS[0];
    expect(pair?.source).toBe('.pi/prompts');
    expect(pair?.mirror).toBe('.claude/commands');
  });
});

describe('listFilesRecursive', () => {
  it('returns empty array for an empty directory', () => {
    const root = tmpDir();
    expect(listFilesRecursive(root)).toEqual([]);
  });

  it('returns a single file at the root', () => {
    const root = tmpDir();
    writeTree(root, { 'foo.md': 'hello' });
    expect(listFilesRecursive(root)).toEqual(['foo.md']);
  });

  it('returns nested files with POSIX separators, sorted', () => {
    const root = tmpDir();
    writeTree(root, {
      'z.md': 'z',
      'a.md': 'a',
      'sub/b.md': 'b',
      'sub/nested/c.md': 'c',
    });
    expect(listFilesRecursive(root)).toEqual(['a.md', 'sub/b.md', 'sub/nested/c.md', 'z.md']);
  });

  it('throws AdwError on symlinks to prevent masquerading content', () => {
    const root = tmpDir();
    const target = join(root, 'real.md');
    writeFileSync(target, 'content');
    symlinkSync(target, join(root, 'link.md'));
    expect(() => listFilesRecursive(root)).toThrow(AdwError);
    expect(() => listFilesRecursive(root)).toThrow(/unexpected non-regular entry/);
  });
});

describe('diffMirror', () => {
  it('returns ok=true and empty diff buckets for identical trees', () => {
    const src = tmpDir();
    const mir = tmpDir();
    writeTree(src, { 'plan.md': 'content', 'sub/review.md': 'review' });
    writeTree(mir, { 'plan.md': 'content', 'sub/review.md': 'review' });
    const result = diffMirror(src, mir);
    expect(result.ok).toBe(true);
    expect(result.missing).toEqual([]);
    expect(result.extra).toEqual([]);
    expect(result.drifted).toEqual([]);
  });

  it('reports files present in source but missing from mirror', () => {
    const src = tmpDir();
    const mir = tmpDir();
    writeTree(src, { 'plan.md': 'content', 'new.md': 'new' });
    writeTree(mir, { 'plan.md': 'content' });
    const result = diffMirror(src, mir);
    expect(result.ok).toBe(false);
    expect(result.missing).toEqual(['new.md']);
    expect(result.extra).toEqual([]);
    expect(result.drifted).toEqual([]);
  });

  it('reports files present in mirror but not in source', () => {
    const src = tmpDir();
    const mir = tmpDir();
    writeTree(src, { 'plan.md': 'content' });
    writeTree(mir, { 'plan.md': 'content', 'stale.md': 'old' });
    const result = diffMirror(src, mir);
    expect(result.ok).toBe(false);
    expect(result.extra).toEqual(['stale.md']);
    expect(result.missing).toEqual([]);
    expect(result.drifted).toEqual([]);
  });

  it('reports files whose bytes differ between source and mirror', () => {
    const src = tmpDir();
    const mir = tmpDir();
    writeTree(src, { 'plan.md': 'version A' });
    writeTree(mir, { 'plan.md': 'version B' });
    const result = diffMirror(src, mir);
    expect(result.ok).toBe(false);
    expect(result.drifted).toEqual(['plan.md']);
    expect(result.missing).toEqual([]);
    expect(result.extra).toEqual([]);
  });

  it('reports all three violation types simultaneously', () => {
    const src = tmpDir();
    const mir = tmpDir();
    writeTree(src, { 'only-src.md': 'x', 'common.md': 'src-version' });
    writeTree(mir, { 'only-mir.md': 'y', 'common.md': 'mir-version' });
    const result = diffMirror(src, mir);
    expect(result.ok).toBe(false);
    expect(result.missing).toEqual(['only-src.md']);
    expect(result.extra).toEqual(['only-mir.md']);
    expect(result.drifted).toEqual(['common.md']);
  });

  it('is sensitive to byte content: trailing newline difference counts as drift', () => {
    const src = tmpDir();
    const mir = tmpDir();
    writeTree(src, { 'a.md': 'hello\n' });
    writeTree(mir, { 'a.md': 'hello' });
    expect(diffMirror(src, mir).drifted).toEqual(['a.md']);
  });

  it('resolves absolute paths unchanged (pass-through for absolute temp dirs)', () => {
    const src = tmpDir();
    const mir = tmpDir();
    writeTree(src, { 'x.md': 'same' });
    writeTree(mir, { 'x.md': 'same' });
    const result = diffMirror(src, mir);
    expect(result.source).toBe(src);
    expect(result.mirror).toBe(mir);
    expect(result.ok).toBe(true);
  });

  // Acceptance criterion: the real .pi/prompts <-> .claude/commands mirror is byte-identical.
  it('real repo mirror is byte-identical (.pi/prompts ↔ .claude/commands)', () => {
    for (const pair of MIRROR_PAIRS) {
      const result = diffMirror(pair.source, pair.mirror);
      if (!result.ok) {
        const lines: string[] = [];
        if (result.missing.length > 0) lines.push(`missing from mirror: ${result.missing.join(', ')}`);
        if (result.extra.length > 0) lines.push(`extra in mirror: ${result.extra.join(', ')}`);
        if (result.drifted.length > 0) lines.push(`drifted: ${result.drifted.join(', ')}`);
        throw new Error(
          `Mirror drift detected (${pair.source} → ${pair.mirror}):\n${lines.join('\n')}\nRun: npm run mirror:sync`,
        );
      }
    }
  });
});
