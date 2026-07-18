/** A machine-readable terminal result for one phased run. */
export type RunOutcomeKind =
  | 'merged'
  | 'pr_ready'
  | 'skipped_closed'
  | 'failed'
  | 'interrupted';

export interface RunOutcome {
  kind: RunOutcomeKind;
  adwId?: string;
  workItemId: string;
  branch?: string;
  changeRequestId?: string;
  changeRequestUrl?: string;
  error?: string;
}
