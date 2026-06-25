/**
 * Provider-neutral work-item helpers.
 *
 * The original implementation lives in issue.ts for backwards compatibility
 * with the historical GitHub-issue workflow. New code should import from this
 * module; issue.ts remains a compatibility surface.
 */

export {
  TYPE_PREFIX,
  branchPrefix,
  deriveBranch,
  deriveWorkItemBranch,
  fetchIssue,
  fetchWorkItem,
  setStatus,
  slugifyTitle,
  slugifyWorkItemTitle,
  workItemBranchPrefix,
  type IssueContext,
  type WorkItemContext,
} from './issue.js';
