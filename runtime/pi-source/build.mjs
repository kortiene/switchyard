// Build the complete Pi Durable workspace dependency closure without changing upstream source.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(fileURLToPath(import.meta.url));
const pins = JSON.parse(readFileSync(join(root, 'pins.json'), 'utf8'));
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
assert.equal(process.versions.node, pins.node, 'Use the exact .node-version');
assert.equal(execFileSync('npm', ['--version'], { encoding: 'utf8' }).trim(), pins.npm, 'Use the pinned npm');
const work = mkdtempSync(join(root, '.build-'));
const source = join(work, 'source');
const packed = join(work, 'packed');
mkdirSync(source);
mkdirSync(packed);
const run = (command, args, cwd = source) => execFileSync(command, args, {
  cwd, stdio: 'inherit', timeout: 600_000,
});
const git = (...args) => execFileSync('git', args, { cwd: source, encoding: 'utf8' }).trim();
try {
  const cache = join(root, '.catalog-cache');
  const catalog = join(cache, `${pins.catalogRevision}.json`);
  if (!existsSync(catalog)) {
    const downloaded = join(work, 'models.all.json');
    run('curl', ['--fail', '--silent', '--show-error', '--location', '--max-time', '90',
      `https://pi.dev/api/models/revisions/${pins.catalogRevision}?types=chat,image,classifier`, '-o', downloaded]);
    assert.equal(`sha256-${sha256(readFileSync(downloaded))}`, pins.catalogRevision, 'Catalog integrity mismatch');
    mkdirSync(cache, { recursive: true });
    renameSync(downloaded, catalog);
  }
  // Cache entries remain untrusted inputs; validate restored bytes on every build.
  assert.equal(`sha256-${sha256(readFileSync(catalog))}`, pins.catalogRevision, 'Catalog integrity mismatch');
  run('git', ['init', '--quiet']);
  run('git', ['fetch', '--depth=1', pins.repository, pins.commit]);
  run('git', ['checkout', '--quiet', '--detach', 'FETCH_HEAD']);
  assert.equal(git('rev-parse', 'HEAD'), pins.commit);
  assert.equal(sha256(readFileSync(join(source, 'package-lock.json'))), pins.lockSha256);
  const manifest = JSON.parse(readFileSync(join(source, 'package.json'), 'utf8'));
  assert.equal(manifest.devDependencies.typescript, pins.typescript);
  const catalogPin = JSON.parse(readFileSync(join(source, 'nix/model-catalog.json'), 'utf8'));
  assert.equal(catalogPin.revision, pins.catalogRevision);

  // Verify all internal production dependencies are built from this source commit.
  const closure = new Set(pins.packages.map(pkg => pkg.name));
  for (const pkg of pins.packages) {
    const metadata = JSON.parse(readFileSync(join(source, 'packages', pkg.directory, 'package.json'), 'utf8'));
    assert.equal(metadata.name, pkg.name);
    assert.equal(metadata.version, pins.version);
    for (const name of Object.keys(metadata.dependencies ?? {})) {
      if (name.startsWith('@earendil-works/')) assert(closure.has(name), `Missing source dependency: ${name}`);
    }
  }
  run('npm', ['ci', '--ignore-scripts', '--no-audit', '--no-fund']);
  run(process.execPath, ['packages/ai/scripts/hydrate-model-catalog.ts', catalog]);
  for (const pkg of pins.packages) {
    run('npm', ['run', pkg.directory === 'ai' ? 'build:offline' : 'build', `--workspace=${pkg.name}`]);
    run('npm', ['pack', '--ignore-scripts', '--silent', `--workspace=${pkg.name}`, `--pack-destination=${packed}`]);
    assert.equal(sha256(readFileSync(join(packed, pkg.artifact))), pkg.sha256, `Artifact drift: ${pkg.name}`);
  }
  assert.equal(git('status', '--porcelain', '--untracked-files=all'), '',
    'Build changed tracked source or created unexpected untracked files');
  assert.equal(sha256(readFileSync(join(source, 'package-lock.json'))), pins.lockSha256);
  const artifacts = join(root, '.artifacts');
  mkdirSync(artifacts, { recursive: true });
  // Promote only after every artifact matches; never rewrite expected hashes during a build.
  for (const pkg of pins.packages) copyFileSync(join(packed, pkg.artifact), join(artifacts, pkg.artifact));
  writeFileSync(join(artifacts, 'provenance.json'), `${JSON.stringify({
    decision: 'NEEDS-FULL-S2', sourceCommit: pins.commit, upstreamLockSha256: pins.lockSha256,
    catalogRevision: pins.catalogRevision, node: pins.node, npm: pins.npm, packages: pins.packages,
  }, null, 2)}\n`);
  console.log('Source artifacts match the reviewed hashes. Full S2 integration remains required.');
} finally {
  rmSync(work, { recursive: true, force: true });
}
