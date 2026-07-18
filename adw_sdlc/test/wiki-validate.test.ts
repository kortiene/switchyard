import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { DEFAULT_WIKI_ROOT, main, validateWiki } from '../tools/wiki-validate.js';

const roots: string[] = [];

function makeWiki(): string {
  const parent = mkdtempSync(join(tmpdir(), 'switchyard-wiki-'));
  roots.push(parent);
  const root = join(parent, 'wiki');
  mkdirSync(root);
  writeFileSync(
    join(root, 'index.md'),
    `---
okf_version: "0.1"
---

# Concepts

* [Example](example.md) - A valid example concept.
`,
  );
  writeFileSync(
    join(root, 'example.md'),
    `---
type: Reference
title: Example
description: A valid example concept.
tags: [example]
timestamp: "2026-07-18T13:26:10Z"
---

See the [bundle index](/index.md).
`,
  );
  return root;
}

afterEach(() => {
  vi.restoreAllMocks();
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe('OKF wiki validation', () => {
  it('accepts a minimal valid bundle', () => {
    const result = validateWiki(makeWiki());
    expect(result.issues).toEqual([]);
    expect(result).toMatchObject({ files: 2, concepts: 1 });
  });

  it('tolerates unknown types and extension fields and ignores non-local example links', () => {
    const root = makeWiki();
    writeFileSync(
      join(root, 'example.md'),
      [
        '---',
        'type: Switchyard-Specific Future Type',
        'extension:',
        '  nested: true',
        '---',
        '',
        '[External](https://example.com/missing)',
        '[Heading](#not-validated)',
        '',
        '````markdown',
        '[Fenced example](/not-created.md)',
        '````',
        '',
      ].join('\n'),
    );

    expect(validateWiki(root).issues).toEqual([]);
  });

  it('checks the committed Switchyard wiki', () => {
    expect(validateWiki(DEFAULT_WIKI_ROOT).issues).toEqual([]);
  });

  it('rejects malformed or missing concept frontmatter', () => {
    const root = makeWiki();
    writeFileSync(join(root, 'example.md'), 'type: Reference\n\nNo delimiters.\n');
    writeFileSync(join(root, 'broken.md'), '---\ntype: [unterminated\n---\n');

    const messages = validateWiki(root).issues.map((item) => `${item.path}: ${item.message}`);
    expect(messages).toEqual(
      expect.arrayContaining([
        expect.stringContaining('broken.md: invalid YAML frontmatter'),
        'example.md: concept must begin with YAML frontmatter',
      ]),
    );
  });

  it('requires a non-empty type and validates recommended field shapes', () => {
    const root = makeWiki();
    writeFileSync(
      join(root, 'example.md'),
      `---
type: ""
title: 42
tags: example
timestamp: 2026-07-18
---
`,
    );

    const messages = validateWiki(root).issues.map((item) => item.message);
    expect(messages).toEqual(
      expect.arrayContaining([
        'frontmatter must contain a non-empty type',
        'title must be a non-empty string when present',
        'tags must be a YAML list of non-empty strings when present',
        'timestamp must be an ISO 8601 datetime string when present',
      ]),
    );
  });

  it('rejects non-mapping frontmatter and invalid UTF-8', () => {
    const root = makeWiki();
    writeFileSync(join(root, 'example.md'), '---\n- Reference\n---\n');
    writeFileSync(join(root, 'invalid-utf8.md'), Buffer.from([0xff, 0xfe, 0xfd]));

    const messages = validateWiki(root).issues.map((item) => `${item.path}: ${item.message}`);
    expect(messages).toEqual(
      expect.arrayContaining([
        expect.stringContaining('example.md: invalid YAML frontmatter: frontmatter must be a YAML mapping'),
        expect.stringContaining('invalid-utf8.md: file is not valid UTF-8'),
      ]),
    );
  });

  it('enforces reserved index and log structure', () => {
    const root = makeWiki();
    const section = join(root, 'section');
    mkdirSync(section);
    writeFileSync(
      join(section, 'index.md'),
      `---
type: Reference
---
# Items
* [Example](/example.md) - Example.
`,
    );
    writeFileSync(
      join(root, 'log.md'),
      `# Bundle log

## 2026-07-17
* **Creation**: Older entry first.

## 2026-07-18
No list entry.
`,
    );

    const messages = validateWiki(root).issues.map((item) => `${item.path}: ${item.message}`);
    expect(messages).toEqual(
      expect.arrayContaining([
        'log.md: log date headings must be newest first',
        'log.md: each log date group must contain at least one list entry',
        'section/index.md: frontmatter is permitted only in the bundle-root index.md',
      ]),
    );
  });

  it('rejects broken bundle-relative and repository-relative links', () => {
    const root = makeWiki();
    writeFileSync(
      join(root, 'example.md'),
      `---
type: Reference
---

[Missing concept](/missing.md)
[Missing source](../missing-source.ts)
`,
    );

    const messages = validateWiki(root).issues.map((item) => item.message);
    expect(messages).toContain('broken local link: /missing.md');
    expect(messages).toContain('broken local link: ../missing-source.ts');
  });

  it('returns stable CLI exit codes for success and invalid arguments', () => {
    const root = makeWiki();
    vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
    vi.spyOn(process.stderr, 'write').mockImplementation(() => true);

    expect(main(['--check', '--root', root])).toBe(0);
    expect(main(['--root'])).toBe(2);
    expect(main(['--unknown'])).toBe(2);
  });
});
