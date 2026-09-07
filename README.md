# CCSDS Ethernet client

This project provides typed, in-process command exchange with a directly
attached CCSDS Ethernet endpoint. The Rust crate owns the full typed session
contract. The optional Python package exposes strict configuration, byte-level
frame helpers, and the Linux raw transport for test orchestrators. It has no
daemon, service, RPC API, sender CLI, plugin system, recording layer,
automatic retry, or UDP/raw fallback.

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

## Rust example

Run the hardware-free example to send one typed command through `Session`,
receive a correlated acknowledgement, and read telemetry that reports an
incremented command counter:

```sh
cargo run --example dummy_ccsds
```

```text
command packet:   11 20 c0 29 00 02 01 00 29
telemetry packet: 01 21 c0 00 00 02 02 00 2a
ack packet:       01 22 c0 00 00 02 03 00 29
acknowledged command counter: 41
telemetry command counter:    42
```

These are complete Space Packets that follow
[CCSDS 133.0-B-2](https://ccsds.org/Pubs/133x0b2e2.pdf), with the mandatory
six-octet primary header. The example uses version `0`, no secondary header,
and unsegmented packets. APID `0x120` carries the dummy telecommand. APID
`0x121` carries the dummy telemetry, and APID `0x122` carries the dummy
acknowledgement. The packet data length is `0x0002`, which means three data
octets because CCSDS encodes this field as the number of data octets minus one.

The application data is intentionally small and mission-specific:

```text
command data:   01 00 29  # increment command, counter 41
telemetry data: 02 00 2a  # counter report, counter 42
```

The application-level command counter is not the primary-header packet
sequence count. The command uses sequence count 41 for illustration. The
telemetry APID has its own sequence count, starting at zero.

The example implements `Codec` in the consuming program and uses
`MemoryTransport` to supply dummy board packets:

```rust
let transport = MemoryTransport::with_incoming([
    incoming_frame(telemetry_packet, board),
    incoming_frame(acknowledgement_packet, board),
]);
let mut session = Session::from_transport(DummyCodec, transport, board)?;

let deadline = Instant::now() + Duration::from_secs(1);
let acknowledgement = session.exchange_once(&command, deadline)?;
let telemetry = session.next_telemetry(deadline)?;
```

See [`examples/dummy_ccsds.rs`](examples/dummy_ccsds.rs) for the complete
packet encoder, decoder, typed codec, exchange, and exact-byte tests. The APIDs
and application data are examples only; a consuming mission crate owns those
definitions.

## Python package

Install `ccsds-ethernet-client` from PyPI on Python 3.11 or newer. Published
wheels target Linux x86-64 and AArch64. The source distribution supports other
Linux targets with a Rust toolchain. Frame helpers also build on other
platforms, but `RawEthernetClient` rejects live use outside Linux.

```python
from ccsds_ethernet_client import RawEthernetClient, RawEthernetConfig

config = RawEthernetConfig(
    interface_name="eth0",
    host_mac="02:00:00:00:00:01",
    host_ip="169.254.209.1",
    host_udp_port=49152,
    board_mac="02:00:00:00:00:7a",
    board_ip="169.254.209.0",
    board_udp_port=24576,
    ring_capacity=64,
)

with RawEthernetClient(config) as client:
    client.send(b"\x10\x01")
    datagram = client.receive(timeout_seconds=0.5)
    print(datagram.payload, datagram.sender_ip)
```

`receive` raises `TimeoutError` when its relative monotonic timeout expires.
Configuration failures raise `ConfigError`. Frame construction and parsing
failures raise `FrameError`. Both are `ValueError` subclasses. Other live
transport failures raise `TransportError`. The package does not encode
commands, correlate acknowledgements, retry, or interpret telemetry. The test
orchestrator owns those policies.

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

For concurrent Python orchestration, `ThreadedRawEthernetClient(config)` owns the
native client on a single thread. It provides `send`, `receive`, `statistics`,
`cancel_receive`, and `close`. Use a context manager to join the owner thread.
Receive and send queues are bounded; `dropped_datagrams` reports receive overflow.
It does not interpret packets or retry sends. The original `RawEthernetClient`
requires serialized access. See [recovery contracts](docs/recovery-contract.md)
for request identity, checksum policy, diagnostics, and recovery semantics.

## Checks

```sh
just check
```

Use `just python-test` to run only the extension build and hardware-free Python
contract tests.

Use `just linux-test` for isolated Linux packet-socket tests. See
[performance evidence](docs/performance.md) for the virtual-link baseline and
the measurements required before throughput optimizations.

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
