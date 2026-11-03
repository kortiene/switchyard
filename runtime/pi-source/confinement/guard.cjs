// Trusted, read-only --require preload, before any adapter/fixture module.
// The loader ignores a missing/invalid LD_PRELOAD library; reject that state.
const { readFileSync } = require('node:fs');
const status = readFileSync('/proc/self/status', 'utf8');
if (process.env.SWITCHYARD_BRIDGE_LOCKDOWN !== 'v1' ||
    !/^NoNewPrivs:\s+1$/m.test(status) || !/^Seccomp:\s+2$/m.test(status)) {
  process.stderr.write('bridge lockdown guard rejected startup\n');
  process.exit(125);
}
// Keep the marker for Node workers, which also run this --require guard.
