/** Durable ownership registry and short filesystem leases for managed runs. */

import { randomUUID } from 'node:crypto';
import {
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { hostname } from 'node:os';
import { dirname, join } from 'node:path';

import { AdwError } from './errors.js';
import { REPO_ROOT } from './common.js';
import { managedControlRoot, type RunContext } from './run-context.js';
import type { RunOutcome } from './run-outcome.js';
import { validateAdwId } from './state.js';

export const MANAGED_RUN_SCHEMA_VERSION = 1;

export type ManagedLifecycleState =
  | 'provisioning'
  | 'ready'
  | 'running'
  | 'merged'
  | 'pr-ready'
  | 'skipped-closed'
  | 'failed'
  | 'interrupted'
  | 'cleanup-needed'
  | 'cleaned'
  | 'orphaned';

export interface ManagedLeaseMetadata {
  leaseId: string;
  pid: number;
  host: string;
  startedAt: string;
  heartbeatAt: string;
}

export interface ManagedRunRecord {
  schemaVersion: 1;
  adwId: string;
  repositoryId: string;
  gitCommonDir: string;
  providerRepo: string;
  workItemProvider: string;
  workItemId: string;
  sourceRoot: string;
  projectRelativePath: string;
  managedRoot: string;
  worktreePath: string;
  projectRoot: string;
  stateRoot: string;
  artifactRoot: string;
  generationId: string;
  branch: string | null;
  branchIntent: string | null;
  base: string;
  allocationOid: string | null;
  expectedHeadOid: string | null;
  remoteHeadOid: string | null;
  lifecycle: ManagedLifecycleState;
  outcome: RunOutcome | null;
  runner: string | null;
  kernelVersion: string;
  configDigest: string | null;
  lease: ManagedLeaseMetadata | null;
  workItemSnapshot: Record<string, unknown> | null;
  changeRequestId: string | null;
  changeRequestUrl: string | null;
  changeRequestHeadOid: string | null;
  mergeIntent: {
    changeRequestId: string;
    headOid: string;
    baseOid: string;
    startedAt: string;
  } | null;
  cleanupDisposition: string | null;
  error: string | null;
  createdAt: string;
  updatedAt: string;
}

interface RegistryDocument extends Omit<ManagedRunRecord, never> {}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function assertRecord(value: unknown, path: string): ManagedRunRecord {
  if (!isObject(value)) {
    throw new AdwError(`corrupt managed run record ${path}: expected an object`);
  }
  const requiredStrings = [
    'adwId',
    'repositoryId',
    'gitCommonDir',
    'providerRepo',
    'workItemProvider',
    'workItemId',
    'sourceRoot',
    'managedRoot',
    'worktreePath',
    'projectRoot',
    'stateRoot',
    'artifactRoot',
    'generationId',
    'base',
    'lifecycle',
    'kernelVersion',
    'createdAt',
    'updatedAt',
  ];
  if (value['schemaVersion'] !== MANAGED_RUN_SCHEMA_VERSION) {
    throw new AdwError(`unsupported managed run record schema in ${path}`);
  }
  for (const key of requiredStrings) {
    if (typeof value[key] !== 'string' || value[key] === '') {
      throw new AdwError(`corrupt managed run record ${path}: invalid ${key}`);
    }
  }
  if (typeof value['projectRelativePath'] !== 'string') {
    throw new AdwError(`corrupt managed run record ${path}: invalid projectRelativePath`);
  }
  validateAdwId(value['adwId'] as string);
  return value as unknown as ManagedRunRecord;
}

function atomicWrite(path: string, value: unknown): void {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
  let fd: number | undefined;
  try {
    fd = openSync(temporary, 'wx', 0o600);
    writeFileSync(fd, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
    closeSync(fd);
    fd = undefined;
    renameSync(temporary, path);
  } catch (error) {
    if (fd !== undefined) closeSync(fd);
    try {
      unlinkSync(temporary);
    } catch {
      // The temporary file may never have been created.
    }
    throw new AdwError(`could not atomically write managed state ${path}`, { cause: error });
  }
}

function processAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM';
  }
}

export interface FileLease {
  readonly metadata: ManagedLeaseMetadata;
  readonly path: string;
  release(): void;
}

/** Acquire an exactly-scoped lease file. A dead same-host owner is reclaimed. */
export function acquireFileLease(path: string): FileLease {
  mkdirSync(dirname(path), { recursive: true });
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const now = new Date().toISOString();
    const metadata: ManagedLeaseMetadata = {
      leaseId: randomUUID(),
      pid: process.pid,
      host: hostname(),
      startedAt: now,
      heartbeatAt: now,
    };
    try {
      const fd = openSync(path, 'wx', 0o600);
      writeFileSync(fd, `${JSON.stringify(metadata)}\n`, 'utf8');
      closeSync(fd);
      let released = false;
      return {
        metadata,
        path,
        release(): void {
          if (released) return;
          released = true;
          let current: unknown;
          try {
            current = JSON.parse(readFileSync(path, 'utf8'));
          } catch {
            return;
          }
          if (isObject(current) && current['leaseId'] === metadata.leaseId) {
            try {
              unlinkSync(path);
            } catch {
              // Idempotent release.
            }
          }
        },
      };
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST') {
        throw new AdwError(`could not acquire managed-run lock ${path}`, { cause: error });
      }
      let owner: unknown;
      try {
        owner = JSON.parse(readFileSync(path, 'utf8'));
      } catch {
        throw new AdwError(`managed-run lock is corrupt: ${path}`);
      }
      if (
        attempt === 0 &&
        isObject(owner) &&
        owner['host'] === hostname() &&
        typeof owner['pid'] === 'number' &&
        !processAlive(owner['pid'])
      ) {
        const stalePath = `${path}.stale.${randomUUID()}`;
        try {
          // Rename is the compare-and-swap: only one contender can move the
          // stale inode, and nobody can accidentally unlink a newly acquired
          // lease at the original path.
          renameSync(path, stalePath);
          unlinkSync(stalePath);
          continue;
        } catch {
          // Another contender reclaimed it first. Retry acquisition once so
          // the new live owner, if any, is reported accurately.
          continue;
        }
      }
      const ownerText = isObject(owner) ? `pid ${String(owner['pid'])} on ${String(owner['host'])}` : 'unknown owner';
      throw new AdwError(`managed-run lock is already held (${ownerText}): ${path}`);
    }
  }
  throw new AdwError(`managed-run lock is already held: ${path}`);
}

export class RunRegistry {
  readonly root: string;

  constructor(readonly gitCommonDir: string) {
    this.root = managedControlRoot(gitCommonDir);
  }

  recordPath(adwId: string): string {
    return join(this.root, 'registry', `${validateAdwId(adwId)}.json`);
  }

  lockPath(kind: 'lifecycle' | 'merge'): string {
    return join(this.root, 'locks', `${kind}.lock`);
  }

  runLockPath(adwId: string): string {
    return join(this.root, 'locks', `run-${validateAdwId(adwId)}.lock`);
  }

  read(adwId: string): ManagedRunRecord | null {
    const path = this.recordPath(adwId);
    let raw: string;
    try {
      raw = readFileSync(path, 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return null;
      throw new AdwError(`could not read managed run record ${path}`, { cause: error });
    }
    let value: unknown;
    try {
      value = JSON.parse(raw);
    } catch (error) {
      throw new AdwError(`corrupt managed run record ${path}: invalid JSON`, { cause: error });
    }
    return assertRecord(value, path);
  }

  list(): ManagedRunRecord[] {
    const directory = join(this.root, 'registry');
    if (!existsSync(directory)) return [];
    return readdirSync(directory)
      .filter((name) => /^[0-9a-f]{8}\.json$/.test(name))
      .sort()
      .map((name) => this.read(name.slice(0, 8)))
      .filter((record): record is ManagedRunRecord => record !== null);
  }

  create(record: ManagedRunRecord): void {
    const path = this.recordPath(record.adwId);
    if (existsSync(path)) {
      throw new AdwError(`managed run ${record.adwId} already exists in this repository`);
    }
    assertRecord(record, path);
    atomicWrite(path, record satisfies RegistryDocument);
  }

  write(record: ManagedRunRecord): void {
    const current = this.read(record.adwId);
    if (current === null) {
      throw new AdwError(`managed run ${record.adwId} has no registry record`);
    }
    if (current.repositoryId !== record.repositoryId || current.generationId !== record.generationId) {
      throw new AdwError(`managed run ${record.adwId} ownership changed while updating its record`);
    }
    assertRecord(record, this.recordPath(record.adwId));
    atomicWrite(this.recordPath(record.adwId), { ...record, updatedAt: new Date().toISOString() });
  }

  update(adwId: string, update: (record: ManagedRunRecord) => ManagedRunRecord): ManagedRunRecord {
    const current = this.read(adwId);
    if (current === null) throw new AdwError(`unknown managed run: ${adwId}`);
    const next = { ...update(structuredClone(current)), updatedAt: new Date().toISOString() };
    this.write(next);
    return next;
  }

  acquireLifecycleLock(): FileLease {
    return acquireFileLease(this.lockPath('lifecycle'));
  }

  acquireMergeLock(): FileLease {
    return acquireFileLease(this.lockPath('merge'));
  }

  acquireRunLease(adwId: string): FileLease {
    return acquireFileLease(this.runLockPath(adwId));
  }
}

export function contextFromRecord(record: ManagedRunRecord): RunContext {
  return Object.freeze({
    packageRoot: REPO_ROOT,
    sourceRoot: record.sourceRoot,
    worktreeRoot: record.worktreePath,
    projectRoot: record.projectRoot,
    stateRoot: record.stateRoot,
    artifactRoot: record.artifactRoot,
    gitCommonDir: record.gitCommonDir,
    mode: 'managed' as const,
  });
}
