import assert from 'node:assert/strict';
import test from 'node:test';
import { assertRustSplit, holdsWorkspaceLock, workspaceLockFilters } from '../rust-suite.mjs';

const serialNames = [
  'service::execution::tests::released_linux_intent_cannot_be_settled_as_unreleased_after_restart',
  'service::execution::tests::receipt_before_lost_reply_replays_and_stale_settlement_cannot_write',
  'service::execution::tests::pi_effects::multi_effect_and_nested_replay_reuse_host_receipts_after_sigkill',
  'service::workspace::tests::recover_still_polls_when_the_workspace_is_busy',
  'service::operator_write::tests::operator_write_lands_the_file_and_a_decision_without_a_revision_bump',
  'service::pi_host_tests::stop_delivery::pi_stops_bypass_blocked_workspaces_without_resuming_pending_work',
  'acceptance::host_verifier::path_gate_uses_the_owned_workspace',
  'abandon_execution_is_audited_idempotently_and_keeps_the_block',
  'supersede_execution_audits_the_simulated_journal_and_refuses_linux_rows',
  'mission_write_copies_one_file_and_refuses_a_closed_phase',
];

const parallelNames = [
  'service::pi_host_tests::lost_ack_recovers_original_receipt_and_identity_across_repeated_restarts',
  'acceptance::host_verifier_confined::host_verifier_confined_pass_admits_complete_pass',
  'typed_effect_responses_preserve_all_four_dispositions',
];

function harness(names: string[], filtered: number) {
  return [
    ...names.map(name => `test ${name} ... ok`),
    `test result: ok. ${names.length} passed; 0 failed; 0 ignored; 0 measured; ${filtered} filtered out; finished in 0.01s`,
    '',
  ].join('\n');
}

test('workspace-lock filters are complements and keep the evidence names', () => {
  for (const name of serialNames) assert.equal(holdsWorkspaceLock(name), true, name);
  for (const name of parallelNames) assert.equal(holdsWorkspaceLock(name), false, name);
  assert.equal(holdsWorkspaceLock('acceptance::host_verifier_confined::host_verifier_confined_pass_admits_complete_pass'), false);

  const parallelOutput = [
    harness(parallelNames, serialNames.length),
    harness(['doc_only_name_without_spaces'], 0),
  ].join('\n');
  const serialOutput = harness(serialNames, parallelNames.length + 1);
  const split = assertRustSplit(parallelOutput, serialOutput, workspaceLockFilters);
  assert.equal(split.rustTests, serialNames.length + parallelNames.length + 1);
  for (const name of serialNames) assert.equal(split.rustPassed.has(name), true, name);
  assert.equal(split.parallel.passed, parallelNames.length + 1);
  assert.equal(split.serial.passed, serialNames.length);

  assert.equal(
    holdsWorkspaceLock('service::execution::tests::pi_effects::uncertain_effect_replay_after_sigkill_never_reapplies_or_unblocks'),
    true,
  );
  assert.equal(
    holdsWorkspaceLock('service::pi_host_tests::lost_ack_recovers_original_receipt_and_identity_across_repeated_restarts'),
    false,
  );
});

test('a dropped, doubled, or failed Rust test refuses the split', () => {
  const okParallel = harness(['keep_parallel'], 1);
  const okSerial = harness(['service::execution::tests::one'], 1);
  assert.throws(
    () => assertRustSplit(okParallel, harness(['service::execution::tests::one', 'extra'], 1), ['service::execution::tests']),
    /serial Rust tests \(2\) are not the tests filtered from the parallel run \(1\)/,
  );
  assert.throws(
    () => assertRustSplit(harness(['service::execution::tests::one'], 0), okSerial, ['service::execution::tests']),
    /serial Rust tests \(1\) are not the tests filtered from the parallel run \(0\)/,
  );
  assert.throws(
    () => assertRustSplit(
      'test result: ok. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out;\n',
      okSerial,
      ['service::execution::tests'],
    ),
    /parallel Rust tests must not fail, ignore, or measure/,
  );
  assert.throws(
    () => assertRustSplit(okParallel, harness(['unrelated'], 1), ['service::execution::tests']),
    /serial Rust run executed no test matching service::execution::tests/,
  );
  assert.throws(
    () => assertRustSplit(
      harness(['keep_parallel', 'shared'], 2),
      harness(['service::execution::tests::one', 'shared'], 2),
      ['service::execution::tests'],
    ),
    /Rust test ran in both invocations: shared/,
  );
});
