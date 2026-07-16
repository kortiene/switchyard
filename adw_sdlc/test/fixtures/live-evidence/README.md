# Live evidence fixtures

These manifests are the sanitized, reviewable record of the live Claude and
cross-language drills run on 2026-07-16 for issues #20–#23.

| Manifest | Observation |
| --- | --- |
| `failure-drills.json` | Real timeout, native budget cap, active-Claude interrupt/restart, completed-phase skip, cleanup, and cost envelopes |
| `secret-boundary.json` | Names-only audits at the executable actually spawned by the Claude Agent SDK |
| `model-routing.json` | All nine configured Claude phase routes, backed by one completed live exemplar per tier |
| `tool-veto.json` | A real Claude Bash request denied by the production git/gh veto, with tool input redacted |
| `merge-refusal.json` | PR #66 creation followed by unattended merge refusal; the PR remained open, green, and unmerged |
| `cross-language-resume.json` | The real Python engine resumed a copy of TypeScript run `c20e5a01` and skipped its completed phase |

## Sanitization boundary

Full prompts, model transcripts, tool inputs, environment values, and absolute
operator paths are intentionally not committed. A retained artifact is instead
represented by its relative role, byte count, and SHA-256 digest. Spawn audits
contain environment key names only. Short log excerpts retain control-flow facts
while omitting prompt bodies and other arguments.

`test/live-evidence.test.ts` guards the expected run IDs, signals, costs, model
routes, exit codes, cleanup observations, PR state, hashes, and redaction rules.
