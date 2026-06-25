import { describe, expect, it } from 'vitest';

import { branchPrefix, deriveBranch, slugifyTitle, type IssueContext, type WorkItemContext } from '../src/issue.js';

describe('WorkItemContext', () => {
  it('is the provider-neutral context shape while IssueContext remains a compatibility alias', () => {
    const item: WorkItemContext = { title: 'T', body: 'B', labels: ['bug'] };
    const issue: IssueContext = item;
    expect(issue).toEqual({ title: 'T', body: 'B', labels: ['bug'] });
  });
});

describe('branchPrefix', () => {
  it('maps type labels with last-match-wins and defaults to feat', () => {
    expect(branchPrefix([])).toBe('feat');
    expect(branchPrefix(['type:bug'])).toBe('fix');
    expect(branchPrefix(['type:feature', 'type:docs'])).toBe('docs');
    expect(branchPrefix(['type:docs', 'type:ci'])).toBe('ci');
  });

  it('maps HealthTech unnamespaced labels case-insensitively', () => {
    expect(branchPrefix(['bug'])).toBe('fix');
    expect(branchPrefix(['Docs'])).toBe('docs');
    expect(branchPrefix(['tech-debt'])).toBe('refactor');
    expect(branchPrefix(['infra'])).toBe('ci');
    expect(branchPrefix(['security'])).toBe('feat'); // unmapped -> default
    expect(branchPrefix(['feature', 'bug'])).toBe('fix'); // last match wins
  });
});

describe('slugifyTitle', () => {
  it('strips the phase prefix, slugifies, and caps at 40 chars', () => {
    expect(slugifyTitle('Phase issue 12: Fix the Frobnicator!')).toBe('fix-the-frobnicator');
    expect(slugifyTitle('  Weird___chars && symbols  ')).toBe('weird-chars-symbols');
    const long = slugifyTitle('a'.repeat(60));
    expect(long.length).toBeLessThanOrEqual(40);
    expect(slugifyTitle('ends with junk!!!')).toBe('ends-with-junk');
  });

  it('strips French diacritics so accented titles slug cleanly', () => {
    expect(slugifyTitle("Dossier d'homologation ARTCI")).toBe('dossier-d-homologation-artci');
    const slug = slugifyTitle('Validation des performances (déchiffrement < 3 s en 3G)');
    expect(slug).toMatch(/^[a-z0-9-]+$/); // fully transliterated, no accents survive
    expect(slug.startsWith('validation-des-performances-dechiffrem')).toBe(true);
    expect(slug.length).toBeLessThanOrEqual(40);
  });
});

describe('deriveBranch', () => {
  it('builds {prefix}/{issue}-[{adw_id}-]{slug}', () => {
    expect(deriveBranch(5, 'Add the thing', ['type:bug'], 'a1b2c3d4')).toBe('fix/5-a1b2c3d4-add-the-thing');
    expect(deriveBranch(5, 'Add the thing', [])).toBe('feat/5-add-the-thing');
  });
});
