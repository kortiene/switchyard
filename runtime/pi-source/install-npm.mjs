// Bootstrap the pinned npm bytes before executing/installing them. No registry metadata is trusted at CI runtime.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { appendFileSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const pins = JSON.parse(readFileSync(new URL('./pins.json', import.meta.url), 'utf8'));
assert.equal(process.versions.node, pins.node, 'Use the exact .node-version');
const directory = mkdtempSync(join(tmpdir(), 'switchyard-npm-'));
try {
  const archive = join(directory, 'npm.tgz');
  execFileSync('curl', ['--fail', '--silent', '--show-error', '--location', '--max-time', '90',
    pins.npmTarball, '-o', archive], { stdio: 'inherit', timeout: 100_000 });
  const integrity = `sha512-${createHash('sha512').update(readFileSync(archive)).digest('base64')}`;
  assert.equal(integrity, pins.npmIntegrity, 'npm tarball integrity mismatch');
  const prefix = process.argv[2] ? resolve(process.argv[2])
    : execFileSync('npm', ['prefix', '--global'], { encoding: 'utf8' }).trim();
  execFileSync('npm', ['install', '--global', '--prefix', prefix, archive,
    '--ignore-scripts', '--no-audit', '--no-fund'], { stdio: 'inherit', timeout: 120_000 });
  const version = execFileSync(process.execPath, [join(prefix, 'lib/node_modules/npm/bin/npm-cli.js'), '--version'],
    { encoding: 'utf8' }).trim();
  assert.equal(version, pins.npm, 'Installed npm version mismatch');
  if (process.env.GITHUB_PATH) appendFileSync(process.env.GITHUB_PATH, `${join(prefix, 'bin')}\n`);
  console.log(`Verified npm ${version}; executable directory: ${join(prefix, 'bin')}`);
} finally {
  rmSync(directory, { recursive: true, force: true });
}
