// S2 feasibility evidence only. This module grants no runtime or mission authority.
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const baseline = {
  sourceCommit: 'e65b3b88c08412bb8ffbd21fb5dc5756064a6e5a',
  publishedSourceCommit: '4a080252f06caf63a525d7873c5d94d3c23887e0',
  version: '1.4.0',
  node: '22.23.4',
  integrity: 'sha512-EtIyXH1MBSXdJSkodmy+CYXuwfve3gPrD4GNCZ0XmuRWLPupwRlNQ7fdaXjRcLoKb6SWkT/yEzK+sMmrqvYK+Q==',
  // Git blob IDs fetched from both exact upstream commits, not inferred from version strings.
  sourceFiles: [
    { name: 'types', approved: 'd7f4a05ec86b1977d2d1c318cded2a192588901f', published: '289b5c4e18b0a36267ef5f5e36d9afda2a649ecf' },
    { name: 'submissions', approved: '66f116af3c5f36547a9a8e0670c334a3d85e800e', published: '0cf838a93932cac6d41ec5e209f987eda1546611' },
  ],
} as const;

export function gitBlobId(source: string): string {
  const bytes = Buffer.from(source);
  return createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex');
}

export async function auditPackage() {
  const require = createRequire(import.meta.url);
  const root = dirname(require.resolve('@earendil-works/pi-durable/package.json'));
  const manifest = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
  const lock = JSON.parse(await readFile(new URL('../package-lock.json', import.meta.url), 'utf8'));
  const locked = lock.packages['node_modules/@earendil-works/pi-durable'];
  const declarations = await readFile(join(root, 'dist/harness/types.d.ts'), 'utf8');
  const sources = [];
  for (const file of baseline.sourceFiles) {
    const map = JSON.parse(await readFile(join(root, `dist/harness/${file.name}.js.map`), 'utf8'));
    const index = map.sources.findIndex((s: string) => s.endsWith(`/harness/${file.name}.ts`));
    if (index < 0 || typeof map.sourcesContent?.[index] !== 'string') {
      throw new Error(`Missing package source evidence: ${file.name}`);
    }
    const actual = gitBlobId(map.sourcesContent[index]);
    sources.push({ path: `packages/durable/src/harness/${file.name}.ts`, actual,
      approved: file.approved, published: file.published,
      matchesApproved: actual === file.approved, matchesPublished: actual === file.published });
  }
  return {
    package: manifest.name,
    version: manifest.version,
    node: process.versions.node,
    expectedNode: baseline.node,
    lockedIntegrity: locked.integrity,
    pinMatches: manifest.version === baseline.version && locked.version === baseline.version
      && locked.integrity === baseline.integrity && process.versions.node === baseline.node,
    approvedSourceCommit: baseline.sourceCommit,
    publishedSourceCommit: baseline.publishedSourceCommit,
    sources,
    declaredCapabilities: {
      transactionalSubmit: /submit\(tx: Tx,/.test(declarations),
      rejectIdleInput: /readonly whenIdle\??:/.test(declarations),
      continueSubmission: /readonly type: "continue"/.test(declarations),
      durableToolTaskId: /readonly taskId: TaskId/.test(declarations),
      nestedToolCalls: /executeTool\(/.test(declarations),
    },
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  console.log(JSON.stringify(await auditPackage(), null, 2));
}
