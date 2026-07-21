/**
 * Human-readable progress copy for public work-item comments.
 *
 * Phase results are model-produced, so free-form summaries, finding text, and
 * paths are deliberately excluded. Only validated enums, booleans, and counts
 * are rendered; this keeps comments useful without echoing arbitrary runner
 * output into a public issue.
 */

import type {
  ClassifyResult,
  DocumentResult,
  E2EResult,
  ImplementResult,
  PatchResult,
  PlanResult,
  ResolveResult,
  ReviewResult,
  TestsResult,
} from './schemas.js';

const PHASE_LABELS: Readonly<Record<string, string>> = {
  ops: 'Run status',
  setup: 'Setup',
  classify: 'Classification',
  plan: 'Planning',
  implement: 'Implementation',
  tests: 'Tests',
  resolve: 'Test resolution',
  e2e: 'End-to-end tests',
  review: 'Review',
  patch: 'Review fixes',
  document: 'Documentation',
  finalize: 'Final verification',
  'ci-fix': 'CI fixes',
  report: 'Run report',
};

const ISSUE_CLASS_LABELS: Readonly<Record<ClassifyResult['issue_class'], string>> = {
  feat: 'feature',
  fix: 'bug fix',
  docs: 'documentation change',
  chore: 'maintenance task',
  ci: 'CI change',
  test: 'testing change',
  refactor: 'refactor',
};

/** Friendly label for built-in and project-defined phase names. */
export function phaseLabel(phase: string): string {
  const known = PHASE_LABELS[phase];
  if (known !== undefined) {
    return known;
  }
  const words = phase.replace(/[-_]+/g, ' ').trim();
  return words === '' ? 'Progress update' : words[0]!.toUpperCase() + words.slice(1);
}

function countLabel(count: number, singular: string, plural: string = `${singular}s`): string {
  return `${count} ${count === 1 ? singular : plural}`;
}

function phaseResultDetail(phase: string, result: unknown): string {
  switch (phase) {
    case 'classify': {
      const value = result as ClassifyResult;
      return `Classified this work as a **${ISSUE_CLASS_LABELS[value.issue_class]}**.`;
    }
    case 'plan': {
      const value = result as PlanResult;
      return value.spec_created
        ? 'Created and validated the implementation plan.'
        : 'Finished the planning pass without creating a new specification.';
    }
    case 'implement': {
      const count = (result as ImplementResult).files_changed.length;
      return count > 0
        ? `Finished the planned implementation across ${countLabel(count, 'changed file')}.`
        : 'Finished the implementation phase; no changed files were reported.';
    }
    case 'tests': {
      const value = result as TestsResult;
      return value.tests_added
        ? 'Added or updated automated test coverage.'
        : 'Completed the test pass without adding new tests.';
    }
    case 'resolve': {
      const value = result as ResolveResult;
      return `Resolved ${countLabel(value.resolved, 'failure')}; ${countLabel(value.remaining, 'failure')} remain.`;
    }
    case 'e2e': {
      const value = result as E2EResult;
      return value.e2e_added
        ? 'Added or updated end-to-end coverage.'
        : 'Completed the end-to-end test pass without adding new coverage.';
    }
    case 'review': {
      const findings = (result as ReviewResult).findings;
      const blockers = findings.filter((finding) => finding.severity === 'blocker').length;
      const followUps = findings.length - blockers;
      if (findings.length === 0) {
        return 'Completed the review with no findings.';
      }
      const parts = [countLabel(blockers, 'blocker'), countLabel(followUps, 'non-blocking finding')]
        .filter((_part, index) => (index === 0 ? blockers > 0 : followUps > 0));
      return `Completed the review and identified ${parts.join(' and ')}.`;
    }
    case 'patch': {
      const value = result as PatchResult;
      return `Resolved ${countLabel(value.resolved, 'review blocker')}; ${countLabel(value.remaining, 'blocker')} remain.`;
    }
    case 'document': {
      const value = result as DocumentResult;
      return value.docs_updated
        ? `Updated documentation across ${countLabel(value.files.length, 'file')}.`
        : 'Completed the documentation pass; no documentation changes were needed.';
    }
    default:
      return 'Completed this phase successfully.';
  }
}

function sentence(text: string): string {
  const clean = text.trim();
  if (clean === '') {
    return '';
  }
  const capitalized = clean[0]!.toUpperCase() + clean.slice(1);
  return /[.!?]$/.test(capitalized) ? capitalized : `${capitalized}.`;
}

function withNext(message: string, nextPhase: string | null | undefined): string {
  if (nextPhase === undefined) {
    return message;
  }
  const next = nextPhase === null ? 'Final verification and merge preparation' : phaseLabel(nextPhase);
  return `${message}\n\n**Next:** ${next}.`;
}

export type PhaseTransitionStatus = 'completed' | 'skipped' | 'blocked';

/** Render a safe, readable transition message with an optional next step. */
export function phaseTransitionProgress(
  status: PhaseTransitionStatus,
  detail: string,
  nextPhase?: string | null,
): string {
  const lead =
    status === 'completed'
      ? '✅ **Completed.**'
      : status === 'skipped'
        ? '⏭️ **Skipped.**'
        : '⚠️ **Blocked.**';
  return withNext(`${lead} ${sentence(detail)}`.trimEnd(), nextPhase);
}

/** Render a phase result using only safe, validated structural fields. */
export function phaseCompletionProgress(
  phase: string,
  result: unknown,
  nextPhase: string | null,
): string {
  return phaseTransitionProgress('completed', phaseResultDetail(phase, result), nextPhase);
}
