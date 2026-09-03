# CCSDS Ethernet client

This crate provides typed, in-process command exchange with a directly
attached CCSDS Ethernet endpoint. It has no daemon, service, RPC API, sender
CLI, Python layer, plugin system, recording layer, automatic retry, or UDP/raw
fallback.

It is available under the [MIT License](LICENSE).

## Contract

`RawEthernetConfig` and its endpoints are immutable after strict construction.
The config requires schema version `2`, caller-supplied concrete unicast IPv4
or IPv6 endpoints, nonzero UDP ports, unicast nonzero MAC addresses, matching
IP address families, distinct host/board identity, a valid Linux interface
name, and a nonzero packet-ring capacity. The crate never infers, discovers,
selects, rewrites, or learns peer addresses.

A mission crate implements `Codec` with its own typed `Command`,
`Acknowledgement`, `Telemetry`, and opaque `Correlation`. `Session::open`
(Linux) or `Session::from_transport` starts receive before any command can be
sent. `exchange_once` accepts an absolute monotonic deadline and performs one
send only. An already-expired deadline is `DeadlineExpiredBeforeSend`; a
receive failure after send is `DeliveryOutcomeUnknown`. Matching busy or
rejected acknowledgements remain successful typed decode results for the
codec/caller to interpret. `next_telemetry` returns telemetry observed while an
exchange waited for its matching acknowledgement. `close` is idempotent.

The Linux transport is cfg-gated and uses an interface-bound `AF_PACKET` raw
socket. Live use requires Linux and `CAP_NET_RAW`; granting that capability and
selecting an interface are deployment responsibilities. Frame/config/ring/
session tests use `MemoryTransport`; default checks never open a NIC or require
root.

`TransportStatistics` reports transport observations only. Detailed frame
classification counters describe what the receive path observed; they do not
decide whether a consumer should accept, reject, persist, or invalidate an
exchange, recording, test run, or safety case. Unsupported EtherTypes continue
to contribute to `ignored_non_ipv4_frames` and also receive a factual detailed
counter. Endpoint mismatches contribute to `foreign_frames`. Parse and
integrity failures contribute to `invalid_frames` and, when the reason is
recognized, the matching detailed failure counter.

Ignored frame payload bytes are not retained. Consumers own serialization,
recording schemas, mission interpretation, and safety policy.

## Checks

```sh
just check
```

## Maintenance

Future agent and maintainer workflow guidance lives in
[`docs/agent-operating-loop.md`](docs/agent-operating-loop.md). Use it to keep
changes aligned with the crate's transport-only boundary and executable test
contracts.

## Scope boundary

Mission APIDs, secondary headers, request-ID allocation, acknowledgement
status policy, retries, recording schemas, logic, decisions, intent, and safety
policy belong to consuming systems.

This crate defines transport and exchange mechanics only; it does not define
subsystem intent or safety policy. Generic file recording is not part of this
crate. Schema version `2` uses a standard 1500-byte Ethernet MTU. IPv4 UDP
payloads are limited to 1472 bytes (1500 minus the 20-byte IPv4 and 8-byte UDP
headers). IPv6 UDP payloads are limited to 1452 bytes (1500 minus the 40-byte
IPv6 and 8-byte UDP headers). Use
`RawEthernetConfig::maximum_udp_payload_bytes()` for the configured family.

See [`docs/release.md`](docs/release.md) for the local release process.
See [`docs/agent-operating-loop.md`](docs/agent-operating-loop.md) for the
agent-facing transport boundary.
