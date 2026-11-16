// Split for the pi-bridge-contract Rust suite. See contract.mjs.
import assert from 'node:assert/strict';

// Tests that flock a workspace owner.lock in the test process. libtest runs
// one binary on many threads; a sibling fork keeps that fd until exec, so the
// parent's close does not release the lock and the next try_lock is
// WorkspaceBusy (#184). Each test already uses its own directory. These
// substrings are the pi-contract tests that take the lock. linux-execution
// tests are not part of this job.
export const workspaceLockFilters = [
  'service::execution::tests',
  'service::workspace::tests',
  'service::operator_write::tests',
  'service::pi_host_tests::stop_delivery',
  'acceptance::host_verifier::',
  'abandon_execution_is_audited_idempotently_and_keeps_the_block',
  'supersede_execution_audits_the_simulated_journal_and_refuses_linux_rows',
  'mission_write_copies_one_file_and_refuses_a_closed_phase',
];

export function rustOutcome(output, label) {
  const summaries = [...output.matchAll(/^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;/gm)];
  assert(summaries.length > 0, `No Rust test summaries found in ${label}`);
  let passed = 0;
  let filtered = 0;
  for (const summary of summaries) {
    assert(summary[2] === '0' && summary[3] === '0' && summary[4] === '0', `${label} must not fail, ignore, or measure`);
    passed += Number(summary[1]);
    filtered += Number(summary[5]);
  }
  const names = [...output.matchAll(/^test (\S+) \.\.\. ok$/gm)].map(match => match[1]);
  return { passed, filtered, names };
}

// libtest runs a test when any positional filter is a substring of its name,
// and skips it when any --skip filter is. The two contract invocations use the
// same strings, so the passed sets are complements.
export function holdsWorkspaceLock(name, filters = workspaceLockFilters) {
  return filters.some(filter => name.includes(filter));
}

export function assertRustSplit(parallelOutput, serialOutput, filters = workspaceLockFilters) {
  const parallel = rustOutcome(parallelOutput, 'parallel Rust tests');
  const serial = rustOutcome(serialOutput, 'serial Rust tests');
  assert.equal(serial.passed, parallel.filtered, `serial Rust tests (${serial.passed}) are not the tests filtered from the parallel run (${parallel.filtered})`);
  assert.equal(parallel.passed, serial.filtered, `parallel Rust tests (${parallel.passed}) are not the tests filtered from the serial run (${serial.filtered})`);
  const serialNames = new Set(serial.names);
  const overlap = parallel.names.filter(name => serialNames.has(name));
  assert.equal(overlap.length, 0, `Rust test ran in both invocations: ${overlap.join(', ')}`);
  for (const filter of filters) {
    assert(serial.names.some(name => name.includes(filter)), `serial Rust run executed no test matching ${filter}`);
  }
  return {
    parallel,
    serial,
    rustPassed: new Set([...parallel.names, ...serial.names]),
    rustTests: parallel.passed + serial.passed,
  };
}
