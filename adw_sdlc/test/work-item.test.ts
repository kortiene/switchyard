import { describe, expect, it } from 'vitest';

import {
  branchPrefix,
  deriveBranch,
  deriveWorkItemBranch,
  fetchIssue,
  fetchWorkItem,
  slugifyWorkItemTitle,
  type IssueContext,
  type WorkItemContext,
  workItemBranchPrefix,
} from '../src/work-item.js';

describe('work-item compatibility surface', () => {
  it('exports provider-neutral helpers and GitHub issue compatibility aliases', () => {
    const item: WorkItemContext = { title: 'T', body: 'B', labels: ['bug'] };
    const issue: IssueContext = item;
    expect(issue.labels).toEqual(['bug']);
    expect(branchPrefix(item.labels)).toBe('fix');
    expect(workItemBranchPrefix(item.labels)).toBe('fix');
    expect(slugifyWorkItemTitle('Work item: Déjà vu')).toBe('work-item-deja-vu');
    expect(deriveBranch(7, item.title, item.labels, 'a1b2c3d4')).toBe('fix/7-a1b2c3d4-t');
    expect(deriveWorkItemBranch(7, item.title, item.labels, 'a1b2c3d4')).toBe('fix/7-a1b2c3d4-t');
    expect(fetchIssue).toBeTypeOf('function');
    expect(fetchWorkItem).toBeTypeOf('function');
  });
});
