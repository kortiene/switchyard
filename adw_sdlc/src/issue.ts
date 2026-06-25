/**
 * Work-item helpers the orchestrator's setup phase needs, ported from the
 * GitHub-issue implementation in adw/work_issue.py: branch derivation from
 * work-item metadata, context fetching (injected into token-less agent phases),
 * and the best-effort project-board/status move. GitHub issue names remain as
 * compatibility aliases while the public provider-neutral shape is
 * WorkItemContext.
 */

import { DEFAULT_ADW_CONFIG, getAdwConfig, type AdwConfig } from './config.js';
import { ghJson, note, runInherit } from './exec.js';

/**
 * Issue label -> branch prefix. Matched case-insensitively. Covers both the
 * mx-agent `type:*` namespace and HealthTech's plain labels (`bug`, `docs`,
 * `tech-debt`, `infra`, ...). Unlisted labels keep the "feat" default.
 */
export const TYPE_PREFIX: Record<string, string> = DEFAULT_ADW_CONFIG.branching.labelPrefixes;

/** Pick a branch prefix from issue labels (last match wins, case-insensitive). */
export function branchPrefix(labels: readonly string[], config: AdwConfig = getAdwConfig()): string {
  let prefix = config.branching.defaultPrefix;
  for (const label of labels) {
    prefix = config.branching.labelPrefixes[label.toLowerCase()] ?? prefix;
  }
  return prefix;
}

/**
 * Slugify an issue title for use in a branch name: strips a leading
 * `Phase issue N:` prefix, lowercases, collapses runs of non-alphanumerics
 * to single hyphens, trims hyphens, and caps at 40 chars.
 */
export function slugifyTitle(title: string, config: AdwConfig = getAdwConfig()): string {
  const slugConfig = config.branching.slug;
  const withoutPrefix = slugConfig.stripPhaseIssuePrefix ? title.replace(/^Phase issue [0-9]+: */, '') : title;
  const normalized = slugConfig.stripDiacritics
    ? withoutPrefix.normalize('NFD').replace(/[\u0300-\u036f]/g, '')
    : withoutPrefix;
  const slug = normalized
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return slug.slice(0, slugConfig.maxLength).replace(/-+$/, '');
}

/**
 * Derive a branch name `{prefix}/{issue}-[{adw_id}-]{slug}` for a phased run.
 * The optional adwId segment correlates the branch with its run state.
 */
export function deriveBranch(
  issue: number,
  title: string,
  labels: readonly string[],
  adwId?: string | null,
  config: AdwConfig = getAdwConfig(),
): string {
  const mid = adwId ? `${adwId}-` : '';
  return `${branchPrefix(labels, config)}/${issue}-${mid}${slugifyTitle(title, config)}`;
}

/** Provider-neutral aliases for branch naming helpers. */
export const workItemBranchPrefix = branchPrefix;
export const slugifyWorkItemTitle = slugifyTitle;
export const deriveWorkItemBranch = deriveBranch;

/** Provider-neutral work-item context injected into token-less agent phases. */
export interface WorkItemContext {
  title: string;
  body: string;
  labels: string[];
}

/** Backward-compatible GitHub issue context alias. */
export type IssueContext = WorkItemContext;

/** Fetch a GitHub-backed work item's title/body/labels via gh, or null if unavailable. */
export function fetchWorkItem(ghBin: string | null, workItem: number, repo: string): WorkItemContext | null {
  if (!ghBin) {
    return null;
  }
  const args = [ghBin, 'issue', 'view', String(workItem)];
  if (repo) {
    args.push('--repo', repo);
  }
  args.push('--json', 'title,body,labels');
  const data = ghJson(args);
  if (typeof data !== 'object' || data === null || Array.isArray(data)) {
    return null;
  }
  const doc = data as Record<string, unknown>;
  const labels = Array.isArray(doc['labels'])
    ? doc['labels'].map((label) =>
        typeof label === 'object' && label !== null
          ? String((label as Record<string, unknown>)['name'] ?? '')
          : '',
      )
    : [];
  return {
    title: typeof doc['title'] === 'string' ? doc['title'] : '',
    body: typeof doc['body'] === 'string' ? doc['body'] : '',
    labels,
  };
}

/** Backward-compatible GitHub issue fetch alias. */
export function fetchIssue(ghBin: string | null, issue: number, repo: string): IssueContext | null {
  return fetchWorkItem(ghBin, issue, repo);
}

function asObject(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/**
 * Best-effort move of the issue's project board card to `targetStatus`
 * (adw/work_issue.py:114-155). PROJECT_NUMBER comes from the parent env.
 * `statusFieldName` defaults to 'Status' (GitHub Projects default).
 */
export function setStatus(
  ghBin: string,
  owner: string,
  issue: number,
  targetStatus: string,
  statusFieldName: string = 'Status',
): void {
  const projectNumber = process.env['PROJECT_NUMBER'] ?? '1';

  const proj = asObject(ghJson([ghBin, 'project', 'view', projectNumber, '--owner', owner, '--format', 'json']));
  const projId = proj?.['id'];
  if (typeof projId !== 'string' || !projId) {
    note('project board not found; skipping status');
    return;
  }

  const items = asObject(
    ghJson([
      ghBin, 'project', 'item-list', projectNumber, '--owner', owner, '--format', 'json', '--limit', '300',
    ]),
  );
  const itemList = Array.isArray(items?.['items']) ? (items['items'] as unknown[]) : [];
  const item = itemList
    .map(asObject)
    .find((it) => it !== null && asObject(it['content'])?.['number'] === issue);
  const itemId = item?.['id'];
  if (typeof itemId !== 'string' || !itemId) {
    note('issue not on board; skipping status');
    return;
  }

  const fields = asObject(ghJson([ghBin, 'project', 'field-list', projectNumber, '--owner', owner, '--format', 'json']));
  const fieldList = Array.isArray(fields?.['fields']) ? (fields['fields'] as unknown[]) : [];
  const statusField = fieldList.map(asObject).find((f) => f !== null && f['name'] === statusFieldName) ?? null;
  const optionList = Array.isArray(statusField?.['options']) ? (statusField['options'] as unknown[]) : [];
  const option = optionList.map(asObject).find((o) => o !== null && o['name'] === targetStatus) ?? null;
  const optionId = option?.['id'];
  if (statusField === null || typeof optionId !== 'string' || !optionId) {
    note(`status option '${targetStatus}' not found; skipping`);
    return;
  }

  const rc = runInherit([
    ghBin, 'project', 'item-edit',
    '--id', itemId,
    '--project-id', projId,
    '--field-id', String(statusField['id']),
    '--single-select-option-id', optionId,
  ]);
  if (rc === 0) {
    note(`set board status of #${issue} -> ${targetStatus}`);
  } else {
    note('could not update board status');
  }
}
