# S3-B6 follow-up: operator model-endpoint wiring

Refs #166, #167. This is CLI wiring for the existing S3-B6 endpoint policy
([s3-model-egress.md](s3-model-egress.md)). It is not a TLS client, not
provider credentials, and not live acceptance. FakeRuntime stays the CLI
default and egress stays denied unless the operator passes the flag.

## Decision

`ConfinedPiConfig.model_endpoint` is host configuration. The CLI takes one
optional global flag, `--model-endpoint <URL>`:

- The flag is valid only with `--runtime confined-pi`. Any other runtime,
  including an omitted flag (FakeRuntime) and an explicit `--runtime fake`,
  is refused while arguments are checked, before `open_host`. The refusal is
  a fixed string and does not echo the value. That check does not create a
  database or take the owner lock.
- Unset keeps the S3-B1 posture: an empty network namespace and the full
  socket deny. Existing runs and the composed acceptance fixture are
  unchanged when the flag is absent. The environment cannot set the
  endpoint. `SWITCHYARD_MODEL_ENDPOINT` is not read.
- On this CLI path the host must be a loopback IP literal: an IPv4 address
  in `127.0.0.0/8`, or IPv6 `::1`, with or without a port. `localhost`, every
  other hostname, and every non-loopback address are refused with no DNS
  lookup. The pin is that literal. `ModelEndpoint::parse` then applies the
  S3-B6 rules to the literal: no userinfo, query, or fragment; HTTPS rejected
  before any resolver call until a reviewed TLS client exists (#166). A
  malformed literal fails closed with that parser's guard detail and does
  not fall back to denied egress. The detail is printed once.
- The same value must be supplied for the mission's lifetime. A later
  command that reopens the host, including `mission command --kind cancel`,
  is refused when the flag is omitted or carries a different value. The
  error names `network`. Re-supplying the original value is the way out.
- Repository files, `case.json`, and protocol frames cannot set it. The URL,
  including its path, is stored in the mission database
  (`attempts.runtime_config_json`, field `network.url`) and in the launch
  record (`egress.url` on the session configuration). It must never carry a
  key. Query, userinfo, and fragment are refused; a key placed in the path
  would still be stored.

## What a CLI run with the flag does not prove

The confined CLI still runs the `s3-acceptance` fixture. That fixture is the
faux provider in `runtime/pi-source/test/bridge-child.ts` (the
`s3-acceptance` responses). Nothing on that path sends a request through
`/model-proxy/endpoint.sock`. Setting `--model-endpoint` covers parse, pin,
and the recorded config only. It is not model-egress evidence and must not
be cited in #102. Real model egress through the CLI needs the S2-1
`product` mode.

## Operator loopback and the signed design

An operator may point the flag at a keyless server on a loopback literal,
for example `http://127.0.0.1:8000/v1`. That route needs no TLS client and
no provider credential. It does not, by itself, exercise bridge egress. See
the fixture limit above.

Design D1–D9 is signed.

- D1(a) is the S2-6 host broker. D1(b) is an operator loopback forwarder.
  Receipts for that forwarder must say "egress pinned by an operator
  forwarder, not by Switchyard".
- D4 is recorded on
  [#167](https://github.com/kortiene/switchyard/issues/167#issuecomment-6954814977).
  The broker text points there. This flag is not that broker, and it does
  not inject credentials.

The operator-run forwarder for that fallback is `tools/model-forwarder/`.
Switchyard does not ship it and does not hold its key. Logs from that
process carry the label "egress pinned by an operator forwarder, not by
Switchyard".

The bridge policy requires Linux x86_64
(`crates/switchyard-core/src/execution/confined_pi/policy.rs`). An aarch64
host can parse and record a loopback literal but cannot launch the confined
bridge.

## Evidence

The default `cli` suite refuses the flag without `--runtime confined-pi`,
refuses a hostname, and refuses a non-loopback address, including an
IPv4-mapped loopback. Those checks create no database and do not need the
confined runtime. `cli_confined` checks that a loopback literal is parsed
before the path prerequisites, that an HTTPS loopback prints the parser's
guard detail once, and that a confined reopen with a different value is
refused with an error that names `network` while the original value is
accepted. The reopen cases need the confined-execution prerequisites, the
same gate as the composed `cli_confined` scenario, and they run in the
`execution-confinement` job. `ModelEndpoint::parse` loopback coverage,
including the `/v1` path form, remains in the S3-B6 suite. These checks are
not live provider calls. A green `execution-confinement` job is evidence
for that revision's parse, pin, and record behavior, not for model egress,
and not for #102.
