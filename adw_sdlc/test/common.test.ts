import { describe, expect, it } from 'vitest';

import { AdwError } from '../src/errors.js';
import { parseJson, shellSplit, stripFrontmatter, substituteArgs } from '../src/common.js';

describe('substituteArgs', () => {
  it('substitutes positionals, $@, $ARGUMENTS, and slices like the Python engine', () => {
    const args = ['12', 'fix', 'the', 'bug'];
    expect(substituteArgs('issue $1: $2', args)).toBe('issue 12: fix');
    expect(substituteArgs('all: $@', args)).toBe('all: 12 fix the bug');
    expect(substituteArgs('all: $ARGUMENTS', args)).toBe('all: 12 fix the bug');
    expect(substituteArgs('rest: ${@:2}', args)).toBe('rest: fix the bug');
    expect(substituteArgs('two: ${@:2:2}', args)).toBe('two: fix the');
  });

  it('renders missing positionals as empty strings', () => {
    expect(substituteArgs('a $1 b $9 c', ['x'])).toBe('a x b  c');
  });
});

describe('stripFrontmatter', () => {
  it('removes a YAML frontmatter block', () => {
    expect(stripFrontmatter('---\ntitle: x\n---\nbody')).toBe('body');
  });

  it('leaves text without frontmatter untouched', () => {
    expect(stripFrontmatter('plain body')).toBe('plain body');
    expect(stripFrontmatter('---\nunterminated')).toBe('---\nunterminated');
  });
});

describe('parseJson', () => {
  it('parses raw JSON', () => {
    expect(parseJson('{"a": 1}')).toEqual({ a: 1 });
  });

  it('prefers the LAST fenced block (agents emit the contract block last)', () => {
    const text = 'intro\n```json\n{"a": 1}\n```\nmore prose\n```json\n{"b": 2}\n```\n';
    expect(parseJson(text)).toEqual({ b: 2 });
  });

  it('accepts a bare ``` fence and JSON embedded in prose', () => {
    expect(parseJson('```\n{"a": 1}\n```')).toEqual({ a: 1 });
    expect(parseJson('the answer is {"a": 1} as shown')).toEqual({ a: 1 });
  });

  it('skips a prose bracket tag before a later JSON object (issue #88)', () => {
    expect(parseJson('[test] completed the work\n{"tests_added":true,"summary":"ok"}', 'object')).toEqual({
      tests_added: true,
      summary: 'ok',
    });
  });

  it('balances nested values and braces inside JSON strings when extracting from prose', () => {
    expect(parseJson('result: {"summary":"kept } literally","items":[{"ok":true}]} done', 'object')).toEqual({
      summary: 'kept } literally',
      items: [{ ok: true }],
    });
  });

  it('prefers an explicit json fence over later-looking generic markdown fences', () => {
    const text = [
      'report',
      '```',
      'tree',
      '```',
      '---',
      '### Current Project Status',
      '```json',
      '{"tests_added": true, "summary": "ok"}',
      '```',
    ].join('\n');
    expect(parseJson(text, 'object')).toEqual({ tests_added: true, summary: 'ok' });
  });

  it('selects the json contract block past tagged+bare fences (issue #35 tests-phase shape)', () => {
    // Reproduces agents/2493f037/tests/transcript-2.log: a long Markdown report
    // with toml/bash/bare fences and a trailing `---`-led section BEFORE the
    // final ```json contract block. The old single-regex parser grabbed the
    // wrong "last fence" (body starting with `---`) and threw
    // "No number after minus sign in JSON".
    const text = [
      'Here is the layout:',
      '```toml',
      '[workspace]',
      'members = ["a", "b"]',
      '```',
      'Run it with:',
      '```bash',
      'cargo test',
      '```',
      'Tree:',
      '```',
      'crates/',
      '```',
      '',
      '---',
      '',
      '### 14. **Current Project Status**',
      '',
      '**Phase:** 0 — spike',
      '',
      '```json',
      '{"tests_added": true, "summary": "Added 14 conformance tests"}',
      '```',
    ].join('\n');
    expect(parseJson(text, 'object')).toEqual({
      tests_added: true,
      summary: 'Added 14 conformance tests',
    });
  });

  it('enforces the expected top-level type', () => {
    expect(() => parseJson('[1, 2]', 'object')).toThrow(AdwError);
    expect(() => parseJson('{"a": 1}', 'array')).toThrow(AdwError);
  });

  it('raises AdwError on empty or unparseable output', () => {
    expect(() => parseJson(null)).toThrow(AdwError);
    expect(() => parseJson('no json here at all')).toThrow(AdwError);
  });
});

describe('shellSplit', () => {
  it('splits on whitespace and honors quotes', () => {
    expect(shellSplit('cargo test --all')).toEqual(['cargo', 'test', '--all']);
    expect(shellSplit("bash -c 'echo a b'")).toEqual(['bash', '-c', 'echo a b']);
    expect(shellSplit('run "two words" x')).toEqual(['run', 'two words', 'x']);
  });
});
