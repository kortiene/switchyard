import { fileURLToPath } from 'node:url';
import { auditPackage } from './package-audit.ts';
import { probeChangedPayload, probeIdleSteering } from './behavior-probes.ts';

// Package compatibility alone never proves the process bridge or the full S2 gate.
export function adoptionDecision(blockers: readonly string[]): 'NO-GO' | 'NEEDS-FULL-S2' {
  return blockers.length > 0 ? 'NO-GO' : 'NEEDS-FULL-S2';
}

export async function feasibilityReport() {
  const packageAudit = await auditPackage();
  const idleSteering = await probeIdleSteering();
  const changedPayload = await probeChangedPayload();
  const blockers = [
    ...(!packageAudit.pinMatches ? ['Installed package/Node does not match the audit pins.'] : []),
    ...(packageAudit.sources.some(source => !source.matchesApproved)
      ? ['Published source/API does not match the reviewed commit; a reviewed compatible pin or source build is required.'] : []),
    ...Object.entries(packageAudit.declaredCapabilities)
      .filter(([, present]) => !present).map(([name]) => `Published API is missing baseline capability: ${name}`),
    ...(!idleSteering.rejected ? ['The idle-steering probe starts a model run instead of rejecting the input.'] : []),
  ];
  return {
    decision: adoptionDecision(blockers),
    scope: 'Published Pi Durable 1.4.0 against the accepted S2 source baseline; not a verdict against Pi or native Rust.',
    packageAudit, idleSteering, changedPayload,
    blockers,
    adapterObligation: 'Same request ID with changed content returns the original Pi submission. A bridge must durably bind and validate the payload hash.',
    unproven: [
      'Rust-to-Pi process protocol, lost-ack reconciliation with the Rust outbox and generation fencing',
      'Stable multi-effect/nested tool identities reaching Rust execution receipts across restart',
      'Bounded transport, snapshot resync, controlled registry and abort settlement contract',
      'S3 operating-system confinement and S8 live acceptance',
    ],
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  console.log(JSON.stringify(await feasibilityReport(), null, 2));
  // Evidence can be collected successfully while adoption remains blocked. This gate
  // deliberately cannot return GO; replace only after the complete S2 contract exists.
  if (process.argv.includes('--gate')) process.exitCode = 1;
}
