# S3-B6 follow-up: operator model-endpoint wiring

Refs #166, #167. This is CLI wiring for the existing S3-B6 endpoint policy
([s3-model-egress.md](s3-model-egress.md)). It is not a TLS client, not
provider credentials, and not live acceptance. FakeRuntime stays the CLI
default and egress stays denied unless the operator acts.

## Decision

`ConfinedPiConfig.model_endpoint` is host configuration. The CLI reads one
optional environment variable, `SWITCHYARD_MODEL_ENDPOINT`:

- Unset keeps the S3-B1 posture: an empty network namespace and the full
  socket deny. Existing runs and the composed acceptance fixture are
  unchanged.
- A set value must parse as one credential-free HTTP model URL under the
  S3-B6 rules: no userinfo, query, or fragment; one resolvable pinned
  address; link-local and cloud-metadata destinations refused; HTTPS
  rejected before DNS until a reviewed TLS client exists (#166). The value
  is parsed and pinned before launch and before the `SWITCHYARD_*` path
  prerequisites, so a malformed endpoint fails closed with its own guard
  error instead of a half-configured run.
- Repository files, `case.json`, and protocol frames cannot set it. The
  launch record keeps the existing egress field: `denied` when unset,
  `model-endpoint` with the URL and pinned address when set.

## Rehearsal route

An operator may point the endpoint at a keyless local OpenAI-compatible
server on loopback, for example `http://127.0.0.1:8000/v1`. That route needs
no TLS client and no provider credential, and it spends nothing. It
exercises bridge egress, dispatch, and recovery without #166 or #167.

- It is not the live-provider acceptance run. #102's ledger cannot present
  it as live acceptance, and #167's recorded provider decision is
  unchanged.
- A server that requires a credential cannot be used directly: the bridge
  must not hold keys. Either the local server runs keyless for the
  rehearsal, or a reviewed host broker (#166 pattern) injects credentials
  outside the bridge; that broker requires its own recorded design first.
- The bridge policy requires Linux x86_64
  (`crates/switchyard-core/src/execution/confined_pi/policy.rs`). An aarch64
  host can parse and record the endpoint but cannot launch the confined
  bridge, so the rehearsal itself runs on an x86_64 Linux host.

## Evidence

`cli_confined` adds two tests that need no confined prerequisites because
the endpoint parses first: an HTTPS value fails closed with the
`SWITCHYARD_MODEL_ENDPOINT` guard, and a credential-free loopback URL
parses and flows past the endpoint check to the missing-prerequisites
guard. `ModelEndpoint::parse` loopback coverage, including the `/v1` path
form, is in the S3-B6 suite. These are local-stub results, not live
provider calls. The required `execution-confinement` job runs
`cli_confined`; a green job is evidence for that revision, not live
acceptance.
