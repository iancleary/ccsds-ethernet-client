# IPv4 UDP checksum qualification

The library implements checksum generation and validation. Peer compatibility
must be qualified with the consuming system's protocol and endpoint configuration.
Keep device configuration, packet captures, and test results outside public source.

## Automated evidence

`just check` compares IPv4 and IPv6 generated checksums with an independent
concatenated pseudo-header oracle at every supported payload length, including
empty, odd, even, and maximum payloads. It also checks corruption of the final
payload byte, Ethernet padding exclusion, and computed-zero encoding as `0xffff`.

`just linux-test` exercises all nine IPv4 sender/receiver policy combinations
over packet sockets. Each combination covers 0, 1, 2, 1471, and 1472 payload bytes.
The matrix below describes acceptance of an otherwise valid, matching frame.

| Sender policy | Legacy receiver | Generate receiver | Require receiver |
| --- | --- | --- | --- |
| Legacy (zero checksum) | Accept | Accept | Count invalid; continue receive |
| Generate (valid checksum) | Accept | Accept | Accept |
| Require (valid checksum) | Accept | Accept | Accept |

An invalid nonzero checksum is rejected by all three policies. A rejected packet
does not end the receive call: the call can return a later valid frame or time out.
These tests establish library behavior through a virtual link. They do not
establish how a physical peer handles generated or missing checksums.

## Physical-peer procedure

1. Record the host software revision, kernel, peer firmware revision, interface
   settings, endpoint addresses, checksum policy, and consumer protocol revision
   in the private test record. Record checksum offload settings when interpreting
   captures; a capture before offload completion may not show the on-wire checksum.
2. Choose a consumer-defined diagnostic command and a distinct request identity.
   Record the expected acknowledgement and telemetry. Define the deadline and
   acceptable loss and latency before the run.
3. Establish the Legacy baseline. Record commands accepted by the local sender,
   commands observed by the peer, acknowledgements, telemetry, and counter deltas.
4. Repeat with Generate. Independently check the transmitted nonzero checksum
   and confirm the peer accepts the packet. Record whether the peer sends zero
   or nonzero response checksums. A successful exchange alone does not prove the
   peer validates corrupted packets.
5. Exercise Require only with a peer configured to send valid checksums. Verify
   that a zero-checksum response is rejected and a later valid response is received.
   Use explicit fault injection supported by the test setup for this step.
6. Inject payload and checksum corruption under controlled test conditions.
   Record receiver counters and peer observations separately. Cover odd and even
   payload lengths and the largest length allowed by the consumer protocol.
7. Repeat after a peer reset and a local close/reopen. Preserve request freshness
   across the reset. Reconcile a timed-out command before any consumer-approved
   resend; transport timeout does not establish that execution failed.
8. Record the qualified policy, firmware/configuration scope, unresolved cases,
   and evidence locations. Keep Legacy as the library default. The consumer owns
   selection of Generate or Require for its deployment.

UDP checksums detect some transmission errors. They do not authenticate peers or
prevent replay. End-to-end integrity and command semantics remain consumer-owned.
