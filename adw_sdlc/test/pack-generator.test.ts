import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { AdwError } from '../src/errors.js';
import {
  checkPack,
  contextHeaderFor,
  generatePack,
  injectContextHeader,
  loadPackProfile,
  renderPackPrompt,
  type PackProfile,
} from '../src/pack-generator.js';

const profile: PackProfile = {
  project: { id: 'demo', name: 'Demo' },
  vars: { project_name: 'Demo' },
  blocks: { domain: 'Project-specific domain guidance for {{ project_name }}.' },
  contextHeader: {
    enabled: true,
    title: '{{ project_name }} context',
    exclude: ['classify'],
    sections: [{ heading: 'Constraints', body: 'Preserve UTF-8: Côte d’Ivoire.' }],
  },
  metaprompt: { enabled: false, instructions: 'Refine.', phaseGuidance: {} },
};

describe('pack prompt generator', () => {
  it('resolves blocks, substitutes vars, injects context after frontmatter, and preserves runtime $ tokens', () => {
    const text = `---\ndescription: Plan for {{ project_name }}\n---\nHello $ARGUMENTS.\n<!-- adw:block domain -->Neutral default.<!-- adw:endblock -->\n`;
    const rendered = renderPackPrompt(text, profile, 'plan.md');
    expect(rendered).toContain('description: Plan for Demo');
    expect(rendered).toMatch(/^---\ndescription: Plan for Demo\n---\n## Demo context/m);
    expect(rendered).toContain('Project-specific domain guidance for Demo.');
    expect(rendered).toContain('Hello $ARGUMENTS.');
    expect(rendered).toContain('Côte d’Ivoire');
    expect(rendered).not.toContain('{{');
    expect(rendered).not.toContain('adw:block');
  });

  it('honors per-phase context-header exclusions', () => {
    expect(contextHeaderFor(profile, 'classify.md')).toBeNull();
    expect(renderPackPrompt('Body for {{ project_name }}', profile, 'classify.md')).toBe('Body for Demo');
  });

  it('fails closed on undefined vars, malformed placeholders, malformed blocks, and frontmatter drift', () => {
    expect(() => renderPackPrompt('Hello {{ missing }}', profile, 'x.md')).toThrow(/undefined template var/);
    expect(() => renderPackPrompt('Hello {{ }}', profile, 'x.md')).toThrow(/residual/);
    expect(() => renderPackPrompt('<!-- adw:block x -->oops', profile, 'x.md')).toThrow(/malformed/);
    expect(() => injectContextHeader('---\nunterminated', profile, 'x.md')).toThrow(/frontmatter/);
  });

  it('generates idempotently and detects drift with --check semantics', () => {
    const root = mkdtempSync(join(tmpdir(), 'adw-pack-gen-'));
    const templates = join(root, 'templates');
    const out = join(root, 'out');
    mkdirSync(templates);
    writeFileSync(join(templates, 'plan.md'), 'Plan for {{ project_name }}\n', 'utf8');

    const first = generatePack({ templatesDir: templates, outDir: out, profile });
    expect(first.written).toEqual(['plan.md']);
    expect(readFileSync(join(out, 'plan.md'), 'utf8')).toContain('Demo');

    const second = generatePack({ templatesDir: templates, outDir: out, profile });
    expect(second.written).toEqual([]);
    expect(second.unchanged).toEqual(['plan.md']);
    expect(checkPack({ templatesDir: templates, outDir: out, profile }).ok).toBe(true);

    writeFileSync(join(out, 'plan.md'), 'drift\n', 'utf8');
    const check = checkPack({ templatesDir: templates, outDir: out, profile });
    expect(check.ok).toBe(false);
    expect(check.drifted).toEqual(['plan.md']);
  });

  it('loads and validates a profile JSON file', () => {
    const root = mkdtempSync(join(tmpdir(), 'adw-pack-profile-'));
    const path = join(root, 'profile.json');
    writeFileSync(path, JSON.stringify({ project: { id: 'p', name: 'Project' } }), 'utf8');
    const loaded = loadPackProfile(path);
    expect(loaded.project.name).toBe('Project');
    expect(loaded.contextHeader.enabled).toBe(true);
    expect(loaded.blocks).toEqual({});

    const bad = join(root, 'bad.json');
    writeFileSync(bad, JSON.stringify({ project: { id: '', name: 'Project' } }), 'utf8');
    expect(() => loadPackProfile(bad)).toThrow(AdwError);
  });
});
