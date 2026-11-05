# S3-B6: model-endpoint-only egress

Refs #153, #97, #94. This is a non-default confinement policy for the
credential-free Pi bridge. FakeRuntime remains the product and CLI default.
Pi is not enabled. Provider credentials, live model calls, #151 cgroup-leaf
cleanup, and composed S3 acceptance stay out of scope.

## Boundary

`ConfinedPiConfig.model_endpoint` is optional host configuration. Unset keeps
the S3-B1 policy: an empty network namespace and a seccomp filter that denies
socket, connect, bind, and the rest of the network calls. That path is what
the existing B1 and Host-dispatch negative tests still run.

When the host sets one endpoint, Rust parses it before launch:

- The value is a single `http` or `https` URL with no userinfo, query, or fragment.
  `https` only defaults the port to 443 and is stored on the session record.
  The proxy copies raw TCP bytes. It does not perform a TLS handshake, send
  SNI, or check a certificate, and this slice has no TLS-over-Unix client.
  A pinned `https` URL is not a finished model route.
- The host resolves it once, outside the bridge, and keeps exactly one address.
- Link-local and cloud-metadata destinations are rejected, including IPv4-mapped
  forms such as `::ffff:169.254.169.254`. Loopback is allowed only as that
  explicit operator address, which is how a local stub stands in for a model
  without live credentials.
- A name that does not resolve to one address is rejected. The pin is not
  refreshed per request.

The bridge does not receive the URL in its environment. It receives a read-only
directory containing one Unix socket, and `SWITCHYARD_BRIDGE_EGRESS=model-socket`.
The host binds that socket in a private `/tmp/sy-<id>` directory. A session
path includes a content hash and does not fit in `sockaddr_un`. The sandbox
path stays `/model-proxy/endpoint.sock`. A hard-killed host can leave those
directories behind. Recovery may remove a `/tmp` directory whose name matches
`sy-` plus 32 hexadecimal characters; it contains only the proxy socket.

The native constructor treats any other value as fatal. Unset still installs
the full socket deny. The filter then allows only `AF_UNIX` `SOCK_STREAM`
`socket` and `connect`. `sendto`, `sendmsg`, `recvfrom`, and `recvmsg` stay
denied, so glibc `send`/`recv` can return `EPERM`; Node `write`/`read` is the
client this filter is built for. `AF_INET`, `AF_INET6`, datagram sockets,
`bind`, `listen`, and `socketpair` stay denied, as do fork and exec. The network
namespace stays empty, so abstract Unix sockets and IP routes cannot reach the
host. The mount namespace contains the proxy socket and no host sockets.

A host thread accepts that socket and splices each connection to the pinned
address. It does not parse HTTP, follow redirects, speak TLS, or dial a host
named in the request. Reads and writes on the splice time out. Closing the
session shuts both ends down and joins the splice threads, which run in the
host rather than in the bridge cgroup.

An environment allowlist or an in-process HTTP wrapper is not this boundary.
Repository files, `case.json`, and protocol frames are not read as endpoint
configuration. `open_confined_pi` only forwards the `ConfinedPiConfig` the
caller already built.

## Evidence

`scripts/bridge-confinement-verify.sh` compiles the same `lockdown.c` and runs
two native binaries: full denial with the variable unset, refusal of any other
value, and `model-socket` allowing a Unix stream socket while IP, bind, and
fork stay denied. The `bridge_egress` test opens a real `ConfinedPiSession`
against a local TCP stub. Inside the bridge it records denial of host TCP, the
unmounted host Unix socket, a loopback listener, metadata and public addresses,
direct TCP to the stub and to a second local port, and DNS lookup. One HTTP
exchange through `/model-proxy/endpoint.sock` returns the stub's token. The
request names the metadata URL; the stub receives those bytes and the second
local listener receives none. The session record stores the operator URL and
the pinned address.

These are fixture and local-stub results. They are not a live provider call.
The required `execution-confinement` job runs this with the existing B1–B5
suites. A green job is evidence for that revision, not full S3 acceptance.
