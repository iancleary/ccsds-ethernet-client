# Exchange and recovery contract

The codec owns request identity and acknowledgement meaning. Transport success
means that the local send operation accepted a frame. It does not prove peer
receipt or command execution.

Allocate a distinct correlation value for each outstanding or potentially stale
request. Equality alone does not prove freshness. If the peer supports epochs,
include a session or boot epoch and request counter in the echoed identity.
Otherwise define a consumer-specific reuse window and restart procedure. Do not
use the CCSDS packet sequence count alone as an execution identity.

Define acknowledgement stages in the consumer protocol: received, accepted,
started, and completed are different observations. Matching rejected or busy
acknowledgements remain typed results for the consumer to interpret.

After a timeout following send, execution remains unknown. Reopening a link does
not resolve that uncertainty. Reconcile peer state or use a protocol-defined
idempotent operation before any resend. The library never retries automatically.

Session statistics retain bounded lifetime counters for decode failures,
unmatched acknowledgements, ignored messages, foreign senders, and dropped
telemetry. Compare snapshots around an exchange to find its observations.
An invalid packet is counted without forcing an unrelated exchange to fail.

Receive readiness means the transport lifecycle permits receive. It does not
assert continuing carrier or peer liveness. Startup revalidates the interface
MAC, name, state, and bound index. Consumers explicitly close and reopen after
identity changes; no peer addresses are learned or rewritten.

IPv4 checksum policy defaults to `Legacy` for wire compatibility. `Generate`
sends checksums while accepting zero-checksum peers. `Require` also rejects
missing receive checksums. Python accepts `legacy`, `generate`, or `require`
as `ipv4_checksum_policy`. Validate peer compatibility before requiring them.
IPv6 always requires UDP checksums. Checksums do not authenticate packets;
consumer protocols own end-to-end integrity and replay protection.

Live transport tests run with `just linux-test` on Linux in a disposable network
namespace. They need iproute2 and namespace permissions, never a physical NIC.
Normal `just check` remains hardware-free and root-free.

The raw Linux transport rejects VLAN frames, including tags stripped into
PACKET_AUXDATA metadata by the NIC. It counts these as `vlan_frames`. Data or
ancillary truncation increments `truncated_frames`. These observations do not
retain ignored payloads. Supporting tagged links requires an explicit future
configuration contract.

The optional Python `ThreadedRawEthernetClient` has a single native owner and
bounded send and receive queues. It receives continuously in 10 ms polling
windows, services one queued send per iteration, and never retries a send.
The polling interval is not a real-time latency guarantee. Current receive
waiters can be cancelled without invalidating subsequent calls. Close wakes
waiters, rejects pending unsent requests, and joins the owner. An already active
send can complete during close. Use a context manager; the owner thread is not
automatically joined by garbage collection. Statistics snapshots include native
idle-poll timeouts; wrapper receive timeouts are separate.

Rust OS failures use `TransportError::Io` with operation, `ErrorKind`, and optional
raw OS error. Native Python exceptions expose `operation`, `kind` (the Rust
ErrorKind name), and `errno`. These fields are `None` for native non-OS failures.
Wrapper-only queue and cancellation errors have no OS evidence; use `getattr`
when inspecting these attributes. Do not parse exception messages for policy.
Send errors remain transport errors; after a successful send, session timeout
or receive failure retains the existing delivery-unknown result. No diagnostic
counter proves command execution.

The new Rust error variants require updates to exhaustive matches. Use a minor
version release for this pre-1.0 API change, not a patch-only release. The default
IPv4 wire format and the serialized Python facade remain unchanged.
