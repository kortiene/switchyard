// Runtime-root digest shared with crates/switchyard-core confined_pi policy.
// Required parts only. Interior symlinks are recorded and not followed.
// The manifest is sorted by raw path bytes, then SHA-256'd.
import { createHash } from 'node:crypto';
import { lstatSync, readdirSync, readFileSync, readlinkSync } from 'node:fs';
import { join } from 'node:path';

const parts = [
  'src',
  'test/bridge-child.ts',
  'node_modules',
  'pins.json',
  'package.json',
  'package-lock.json',
  'confinement/guard.cjs',
  '.artifacts/provenance.json',
];

function walk(root, rel, entries) {
  const abs = join(root, rel);
  const stat = lstatSync(abs);
  if (stat.isSymbolicLink()) {
    const target = readlinkSync(abs);
    if (target.includes('\n') || target.includes('\0')) {
      throw new Error(`bridge runtime symlink target is not a single line: ${rel}`);
    }
    entries.push([rel, `l ${rel} ${target}`]);
    return;
  }
  if ((stat.mode & 0o6022) !== 0) {
    throw new Error(`bridge runtime component ${rel} is setid or group/world writable`);
  }
  if (stat.isDirectory()) {
    for (const name of readdirSync(abs)) {
      walk(root, `${rel}/${name}`, entries);
    }
    return;
  }
  if (stat.isFile()) {
    const hash = createHash('sha256').update(readFileSync(abs)).digest('hex');
    entries.push([rel, `f ${rel} ${hash}`]);
    return;
  }
  throw new Error(`bridge runtime component ${rel} is a special file`);
}

const root = process.argv[2];
if (!root) {
  throw new Error('usage: node scripts/runtime-root-digest.mjs <runtime-root>');
}
const entries = [];
for (const part of parts) walk(root, part, entries);
entries.sort((left, right) => Buffer.compare(Buffer.from(left[0]), Buffer.from(right[0])));
const manifest = entries.map((entry) => `${entry[1]}\n`).join('');
process.stdout.write(`sha256:${createHash('sha256').update(manifest).digest('hex')}\n`);
