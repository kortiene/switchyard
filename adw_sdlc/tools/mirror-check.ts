#!/usr/bin/env node
/**
 * Check (default) or repair the neutral-prompt mirror.
 *
 * The repo keeps `.pi/prompts` (canonical) byte-for-byte identical to
 * `.claude/commands` (hand-maintained mirror). This CLI is the gate-enforced
 * guard for that invariant — `--check` is the deterministic, CI-safe default
 * (writes nothing, exits non-zero on any drift) and is wired into `npm run
 * verify` next to `pack:check`. `--write` repairs drift by making the mirror
 * match the canonical source exactly.
 *
 * Like every ADW phase tool it does NO git and NO network — it only reads/writes
 * the two local prompt trees. The orchestrator owns all git/gh.
 */

import { mkdirSync, readFileSync, readdirSync, rmdirSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { AdwError } from '../src/errors.js';
import { diffMirror, listFilesRecursive, MIRROR_PAIRS, type MirrorResult } from './mirror.js';

const USAGE = `usage: mirror-check [--check] [--write] [--dry-run] [-h|--help]

Check (default) or repair the .pi/prompts <-> .claude/commands byte-identical mirror.
The canonical source is .pi/prompts; --write makes .claude/commands match it.

Flags:
  --check     verify the mirror is byte-identical; write nothing (default)
  --write     copy the canonical source tree onto the mirror to fix drift
  --dry-run   with --write, show what would change; write nothing
  -h, --help  show this help`;

interface Args {
  write: boolean;
  dryRun: boolean;
}

function parseArgs(argv: readonly string[]): Args {
  const args: Args = { write: false, dryRun: false };
  for (const arg of argv) {
    switch (arg) {
      case '-h':
      case '--help':
        process.stdout.write(`${USAGE}\n`);
        process.exit(0);
      case '--check':
        // The default; accepted explicitly so `mirror:check` reads clearly.
        break;
      case '--write':
        args.write = true;
        break;
      case '--dry-run':
        args.dryRun = true;
        break;
      default:
        throw new AdwError(`unknown flag: ${arg}`);
    }
  }
  return args;
}

/** Print the drift buckets for one pair to stderr (used by `--check` failures). */
function reportDrift(result: MirrorResult): void {
  const pair = `${result.source} -> ${result.mirror}`;
  if (result.missing.length > 0) {
    process.stderr.write(`missing from mirror (${pair}): ${result.missing.join(', ')}\n`);
  }
  if (result.extra.length > 0) {
    process.stderr.write(`extra in mirror (${pair}): ${result.extra.join(', ')}\n`);
  }
  if (result.drifted.length > 0) {
    process.stderr.write(`drifted (${pair}): ${result.drifted.join(', ')}\n`);
  }
}

interface SyncResult {
  synced: string[]; // source files written/overwritten into the mirror
  unchanged: string[]; // source files already byte-identical in the mirror
  removed: string[]; // extra mirror files deleted
}

/**
 * Make the mirror byte-identical to the source: write every missing/drifted
 * source file, delete every extra mirror file, then prune now-empty mirror
 * directories. Idempotent — a clean mirror yields empty `synced`/`removed`.
 * `dryRun` reports the same buckets without touching disk.
 */
function syncMirror(result: MirrorResult, dryRun: boolean): SyncResult {
  const toWrite = [...result.missing, ...result.drifted].sort();
  const srcFiles = listFilesRecursive(result.source);
  const unchanged = srcFiles.filter((f) => !result.missing.includes(f) && !result.drifted.includes(f));

  if (!dryRun) {
    for (const rel of toWrite) {
      const dest = join(result.mirror, rel);
      mkdirSync(dirname(dest), { recursive: true });
      writeFileSync(dest, readFileSync(join(result.source, rel)));
    }
    for (const rel of result.extra) {
      unlinkSync(join(result.mirror, rel));
    }
    pruneEmptyDirs(result.mirror);
  }

  return { synced: toWrite, unchanged, removed: [...result.extra] };
}

/** Recursively remove empty subdirectories under `root` (leaves `root` itself). */
function pruneEmptyDirs(root: string): void {
  const walk = (abs: string): boolean => {
    let empty = true;
    for (const entry of readdirSync(abs, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        const childEmpty = walk(join(abs, entry.name));
        empty = empty && childEmpty;
      } else {
        empty = false;
      }
    }
    if (empty && abs !== root) {
      rmdirSync(abs);
    }
    return empty;
  };
  walk(root);
}

export async function main(argv: readonly string[]): Promise<number> {
  try {
    const args = parseArgs(argv);
    const results = MIRROR_PAIRS.map((pair) => diffMirror(pair.source, pair.mirror));

    if (!args.write) {
      const drifted = results.filter((r) => !r.ok);
      if (drifted.length === 0) {
        process.stdout.write('mirror is byte-identical\n');
        return 0;
      }
      for (const result of drifted) {
        reportDrift(result);
      }
      process.stderr.write('run: npm run mirror:sync\n');
      return 1;
    }

    let synced = 0;
    let unchanged = 0;
    let removed = 0;
    for (const result of results) {
      const sync = syncMirror(result, args.dryRun);
      synced += sync.synced.length;
      unchanged += sync.unchanged.length;
      removed += sync.removed.length;
    }
    const prefix = args.dryRun ? 'would sync' : 'synced';
    process.stdout.write(`${prefix}: ${synced}; unchanged: ${unchanged}; removed: ${removed}\n`);
    return 0;
  } catch (err) {
    if (err instanceof AdwError) {
      process.stderr.write(`error: ${err.message}\n`);
      return 2;
    }
    throw err;
  }
}

if (process.argv[1] !== undefined && process.argv[1] === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).then((rc) => {
    process.exitCode = rc;
  });
}
