/**
 * Persistent run state, ported from adw/_state.py. The on-disk shape —
 * agents/{adw_id}/state.json plus per-phase artifact dirs — is codified in
 * adw/state.schema.json (the cross-language contract from PLAN.md D4) and
 * validated by the TS state/parity tests against the committed fixtures. The
 * Python engine is not bundled in this standalone port.
 *
 * Canonical v1 fields are written always (including nulls), exactly like the
 * Python dataclass writer; TS-only additions (engine/runner/total_cost_usd,
 * merge_skipped, and provider-neutral work_item/change_request metadata) are
 * additive, written only when set, and never load-bearing for resume —
 * Python's reader drops them, and resume works from v1 fields +
 * completed_phases alone.
 */

import { randomBytes } from 'node:crypto';
import { mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { projectRoot } from './common.js';
import { AdwError } from './errors.js';

export const STATE_FILENAME = 'state.json';

const ADW_ID_RE = /^[0-9a-f]{8}$/;
const PHASE_RE = /^[a-zA-Z0-9_-]+$/;

// Overridable for tests (the analogue of patching _state.AGENTS_DIR).
let agentsDirOverride: string | null = null;

/**
 * The workspace root agents/ — under the project root (the target repo when an
 * explicit project root is set, else the package root), test-overridable via
 * setAgentsDir. The explicit override always wins (AC4's "unless explicitly
 * overridden").
 */
export function agentsDir(): string {
  return agentsDirOverride ?? join(projectRoot(), 'agents');
}

/** Override the workspace root (tests); pass null to restore the default. */
export function setAgentsDir(dir: string | null): void {
  agentsDirOverride = dir;
}

/** Generate a short 8-character hex run id (e.g. `a1b2c3d4`). */
export function makeAdwId(): string {
  return randomBytes(4).toString('hex');
}

/**
 * Return `adwId` if it is a valid 8-char hex id, else raise AdwError. Guards
 * against path injection before adwId is used as a path segment under agents/.
 */
export function validateAdwId(adwId: string): string {
  if (!adwId || !ADW_ID_RE.test(adwId)) {
    throw new AdwError(`invalid adw_id (want 8 hex chars): ${JSON.stringify(adwId)}`);
  }
  return adwId;
}

function safePhase(phase: string): string {
  if (!phase || !PHASE_RE.test(phase)) {
    throw new AdwError(`invalid phase name: ${JSON.stringify(phase)}`);
  }
  return phase;
}

/** Persisted review finding; additive keys from any writer are preserved. */
export type FindingRecord = Record<string, unknown>;

export type StateMetadata = Record<string, unknown>;

export interface AdwStateInit {
  adwId: string;
  schemaVersion?: number;
  issueNumber?: string | null;
  issueClass?: string | null;
  branchName?: string | null;
  base?: string;
  planFile?: string | null;
  prNumber?: number | null;
  prUrl?: string | null;
  commitMessage?: string | null;
  prBody?: string | null;
  reviewFindings?: FindingRecord[];
  completedPhases?: string[];
  engine?: string;
  runner?: string;
  /** Provider-neutral work item metadata, additive and non-load-bearing. */
  workItem?: StateMetadata;
  /** Provider-neutral change-request metadata, additive and non-load-bearing. */
  changeRequest?: StateMetadata;
  /** Accumulated run cost; null = some phase could not be priced (unknown). */
  totalCostUsd?: number | null;
  /** Why the merge stage was intentionally skipped; absent once merged. */
  mergeSkipped?: 'flag';
  /** Exact durable workspace for managed mode; omitted keeps agents/<id>. */
  controlRoot?: string;
  /** Worktree-local agent-authored artifact directory; defaults to controlRoot. */
  artifactRoot?: string;
  /** Managed state is load-bearing: use atomic writes and surface I/O failures. */
  strictPersistence?: boolean;
}

export interface AdwStateLocation {
  controlRoot: string;
  artifactRoot: string;
  strictPersistence?: boolean;
}

/** Minimal persistent state connecting phased-run steps (adw/_state.py:51). */
export class AdwState {
  adwId: string;
  schemaVersion: number;
  issueNumber: string | null;
  issueClass: string | null;
  branchName: string | null;
  base: string;
  planFile: string | null;
  prNumber: number | null;
  prUrl: string | null;
  commitMessage: string | null;
  prBody: string | null;
  reviewFindings: FindingRecord[];
  completedPhases: string[];
  // TS-additive fields (PLAN.md D4): recorded for observability, dropped by
  // the Python reader, never load-bearing for resume.
  engine: string | undefined;
  runner: string | undefined;
  workItem: StateMetadata | undefined;
  changeRequest: StateMetadata | undefined;
  /** Accumulated run cost; null = some phase could not be priced (unknown). */
  totalCostUsd: number | null | undefined;
  /** Additive PR-only bookkeeping; merge remains incomplete and resumable. */
  mergeSkipped: 'flag' | undefined;
  private readonly controlRoot: string | undefined;
  private readonly artifactRoot: string | undefined;
  private readonly strictPersistence: boolean;

  constructor(init: AdwStateInit) {
    this.adwId = validateAdwId(init.adwId);
    this.schemaVersion = init.schemaVersion ?? 1;
    this.issueNumber = init.issueNumber ?? null;
    this.issueClass = init.issueClass ?? null;
    this.branchName = init.branchName ?? null;
    this.base = init.base ?? 'main';
    this.planFile = init.planFile ?? null;
    this.prNumber = init.prNumber ?? null;
    this.prUrl = init.prUrl ?? null;
    this.commitMessage = init.commitMessage ?? null;
    this.prBody = init.prBody ?? null;
    this.reviewFindings = init.reviewFindings ?? [];
    this.completedPhases = init.completedPhases ?? [];
    this.engine = init.engine;
    this.runner = init.runner;
    this.workItem = init.workItem;
    this.changeRequest = init.changeRequest;
    this.totalCostUsd = init.totalCostUsd;
    this.mergeSkipped = init.mergeSkipped;
    this.controlRoot = init.controlRoot;
    this.artifactRoot = init.artifactRoot;
    this.strictPersistence = init.strictPersistence ?? false;
  }

  // --- paths -----------------------------------------------------------------

  /** This run's workspace directory agents/{adw_id}/. */
  workspace(): string {
    return this.controlRoot ?? join(agentsDir(), this.adwId);
  }

  /** Directory whose paths may be handed to the coding agent. */
  artifactWorkspace(): string {
    return this.artifactRoot ?? this.workspace();
  }

  /** The path to this run's state.json. */
  statePath(): string {
    return join(this.workspace(), STATE_FILENAME);
  }

  /** Return (creating) the per-phase artifact directory for `phase`. */
  phaseDir(phase: string): string {
    const directory = join(this.workspace(), safePhase(phase));
    mkdirSync(directory, { recursive: true });
    return directory;
  }

  // --- phase bookkeeping -------------------------------------------------------

  /** Whether `phase` has already completed in this run. */
  isDone(phase: string): boolean {
    return this.completedPhases.includes(phase);
  }

  /** Record `phase` as completed (idempotent). */
  markDone(phase: string): void {
    if (!this.completedPhases.includes(phase)) {
      this.completedPhases.push(phase);
    }
  }

  // --- persistence ---------------------------------------------------------------

  /** The on-disk (snake_case) document; v1 fields always present. */
  toJSON(): Record<string, unknown> {
    const doc: Record<string, unknown> = {
      adw_id: this.adwId,
      schema_version: this.schemaVersion,
      issue_number: this.issueNumber,
      issue_class: this.issueClass,
      branch_name: this.branchName,
      base: this.base,
      plan_file: this.planFile,
      pr_number: this.prNumber,
      pr_url: this.prUrl,
      commit_message: this.commitMessage,
      pr_body: this.prBody,
      review_findings: this.reviewFindings,
      completed_phases: this.completedPhases,
    };
    if (this.engine !== undefined) {
      doc['engine'] = this.engine;
    }
    if (this.runner !== undefined) {
      doc['runner'] = this.runner;
    }
    if (this.workItem !== undefined) {
      doc['work_item'] = this.workItem;
    }
    if (this.changeRequest !== undefined) {
      doc['change_request'] = this.changeRequest;
    }
    if (this.totalCostUsd !== undefined) {
      doc['total_cost_usd'] = this.totalCostUsd;
    }
    if (this.mergeSkipped !== undefined) {
      doc['merge_skipped'] = this.mergeSkipped;
    }
    return doc;
  }

  /**
   * Persist state. Legacy state remains best-effort for cross-language parity;
   * managed state uses a unique temporary plus atomic rename and fails loudly.
   */
  save(): void {
    try {
      const path = this.statePath();
      mkdirSync(this.workspace(), { recursive: true });
      if (!this.strictPersistence) {
        writeFileSync(path, `${JSON.stringify(this.toJSON(), null, 2)}\n`, 'utf8');
        return;
      }
      const temporary = `${path}.${process.pid}.${randomBytes(8).toString('hex')}.tmp`;
      try {
        writeFileSync(temporary, `${JSON.stringify(this.toJSON(), null, 2)}\n`, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
        renameSync(temporary, path);
      } catch (error) {
        try {
          unlinkSync(temporary);
        } catch {
          // The temporary may not have been created.
        }
        throw error;
      }
    } catch (error) {
      if (this.strictPersistence) {
        throw new AdwError(`could not persist managed run state ${this.statePath()}`, { cause: error });
      }
      // Legacy state is intentionally best effort.
    }
  }

  /** Load state for `adwId`, or null if it is missing or unreadable. */
  static load(adwId: string): AdwState | null {
    return AdwState.loadFrom(adwId, undefined, false);
  }

  /** Managed reader: missing is null, while corrupt/unreadable is a hard error. */
  static loadManaged(adwId: string, location: AdwStateLocation): AdwState | null {
    return AdwState.loadFrom(adwId, location, true);
  }

  private static loadFrom(adwId: string, location: AdwStateLocation | undefined, strict: boolean): AdwState | null {
    validateAdwId(adwId);
    const path = join(location?.controlRoot ?? join(agentsDir(), adwId), STATE_FILENAME);
    let raw: string;
    try {
      raw = readFileSync(path, 'utf8');
    } catch (error) {
      if (strict && (error as NodeJS.ErrnoException).code !== 'ENOENT') {
        throw new AdwError(`could not read managed run state ${path}`, { cause: error });
      }
      return null;
    }
    let data: unknown;
    try {
      data = JSON.parse(raw);
    } catch (error) {
      if (strict) throw new AdwError(`corrupt managed run state ${path}: invalid JSON`, { cause: error });
      return null;
    }
    if (typeof data !== 'object' || data === null || Array.isArray(data)) {
      if (strict) throw new AdwError(`corrupt managed run state ${path}: expected an object`);
      return null;
    }
    const doc = data as Record<string, unknown>;
    if (typeof doc['adw_id'] !== 'string') {
      if (strict) throw new AdwError(`corrupt managed run state ${path}: missing adw_id`);
      return null;
    }
    // Forward-compatible like the Python reader: keep only declared fields,
    // ignore unknown keys, tolerate junk by falling back to defaults.
    try {
      return new AdwState({
        adwId: doc['adw_id'],
        schemaVersion: typeof doc['schema_version'] === 'number' ? doc['schema_version'] : 1,
        issueNumber: asWorkItemIdStringOrNull(doc['issue_number']),
        issueClass: asStringOrNull(doc['issue_class']),
        branchName: asStringOrNull(doc['branch_name']),
        base: typeof doc['base'] === 'string' ? doc['base'] : 'main',
        planFile: asStringOrNull(doc['plan_file']),
        prNumber: typeof doc['pr_number'] === 'number' ? doc['pr_number'] : null,
        prUrl: asStringOrNull(doc['pr_url']),
        commitMessage: asStringOrNull(doc['commit_message']),
        prBody: asStringOrNull(doc['pr_body']),
        reviewFindings: Array.isArray(doc['review_findings'])
          ? doc['review_findings'].filter(isPlainObject)
          : [],
        completedPhases: Array.isArray(doc['completed_phases'])
          ? doc['completed_phases'].filter((p): p is string => typeof p === 'string')
          : [],
        engine: typeof doc['engine'] === 'string' ? doc['engine'] : undefined,
        runner: typeof doc['runner'] === 'string' ? doc['runner'] : undefined,
        workItem: isPlainObject(doc['work_item']) ? doc['work_item'] : undefined,
        changeRequest: isPlainObject(doc['change_request']) ? doc['change_request'] : undefined,
        totalCostUsd:
          typeof doc['total_cost_usd'] === 'number' || doc['total_cost_usd'] === null
            ? doc['total_cost_usd']
            : undefined,
        mergeSkipped: doc['merge_skipped'] === 'flag' ? 'flag' : undefined,
        ...(location !== undefined
          ? {
              controlRoot: location.controlRoot,
              artifactRoot: location.artifactRoot,
              strictPersistence: location.strictPersistence ?? true,
            }
          : {}),
      });
    } catch (error) {
      if (strict) throw new AdwError(`corrupt managed run state ${path}: invalid fields`, { cause: error });
      return null;
    }
  }
}

function asStringOrNull(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}

/** Canonicalize legacy numeric issue_number values for safe cross-version resume. */
function asWorkItemIdStringOrNull(value: unknown): string | null {
  if (typeof value === 'string') {
    return value;
  }
  return typeof value === 'number' && Number.isInteger(value) ? String(value) : null;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
