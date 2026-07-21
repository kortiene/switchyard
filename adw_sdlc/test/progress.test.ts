import { describe, expect, it } from 'vitest';

import { formatProgress } from '../src/exec.js';
import {
  phaseCompletionProgress,
  phaseLabel,
  phaseTransitionProgress,
} from '../src/progress.js';

describe('progress comments', () => {
  it('keeps the stable run marker and adds a readable Markdown heading', () => {
    expect(formatProgress('a1b2c3d4', 'ci-fix', 'CI is green.', '[ACME-ADW]')).toBe(
      '[ACME-ADW] a1b2c3d4_ci-fix:\n\n### CI fixes\n\nCI is green.',
    );
  });

  it('renders a meaningful completion and next phase from safe result fields', () => {
    expect(
      phaseCompletionProgress(
        'review',
        {
          findings: [
            { severity: 'blocker', description: 'private detail', location: 'private/path.ts:1' },
            { severity: 'tech_debt', description: 'another private detail', location: '' },
          ],
          wrote_commit_message: true,
          wrote_pr_body: true,
        },
        'patch',
      ),
    ).toBe(
      '✅ **Completed.** Completed the review and identified 1 blocker and 1 non-blocking finding.\n\n' +
        '**Next:** Review fixes.',
    );
  });

  it('never echoes free-form summaries, finding text, or paths into public progress', () => {
    const secret = 'do-not-publish-this-token';
    const implementation = phaseCompletionProgress(
      'implement',
      { summary: secret, files_changed: [`private/${secret}.ts`] },
      'tests',
    );
    const review = phaseCompletionProgress(
      'review',
      {
        findings: [{ severity: 'blocker', description: secret, location: secret }],
        wrote_commit_message: true,
        wrote_pr_body: true,
      },
      'patch',
    );

    expect(`${implementation}\n${review}`).not.toContain(secret);
    expect(implementation).toContain('1 changed file');
    expect(review).toContain('1 blocker');
  });

  it('explains skipped and blocked transitions in plain language', () => {
    expect(phaseTransitionProgress('skipped', 'no documentation changes were detected', 'review')).toBe(
      '⏭️ **Skipped.** No documentation changes were detected.\n\n**Next:** Review.',
    );
    expect(phaseTransitionProgress('blocked', '2 test failures remain', null)).toBe(
      '⚠️ **Blocked.** 2 test failures remain.\n\n**Next:** Final verification and merge preparation.',
    );
  });

  it('humanizes project-defined phase names', () => {
    expect(phaseLabel('security_audit')).toBe('Security audit');
  });
});
