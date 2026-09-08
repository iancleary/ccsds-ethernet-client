# Transport workload benchmark

This standalone Rust example measures a workload generator and synthetic peer.
It does not implement an application command protocol. It does not change the
library, retry commands, choose physical interfaces, or publish results.

## Run

Run the root-free simulator:

```sh
just benchmark --rates 100,1000,10000 --samples 1000 --payload-bytes 256
```

Run the same generator through the actual Rust packet transport on Linux:

```sh
just linux-benchmark --rates 100,1000,10000 --samples 1000 --payload-bytes 256
```

The Linux runner creates and deletes its own network namespace and veth pair.
It needs iproute2 and root or noninteractive sudo. It builds as the invoking user
and runs only the built executable inside the fixture. Do not invoke the veth
backend against pre-existing interfaces. No physical-interface mode is provided.

Both recipes use optimized builds and emit JSON Lines to stdout. Each rate
produces one `trial` record. A final `sweep_summary` follows. Progress and errors
use stderr. Invalid configuration exits nonzero before any trial runs. Setup or
fatal errors exit nonzero; previously emitted trials remain valid, but a missing
summary means the sweep did not finish. Observed loss is data, not a failed CLI.

Examples of distinct workloads:

```sh
# Sparse commands plus independently paced background telemetry.
just benchmark --rates 10 --samples 100 --telemetry-per-command 0 --telemetry-rate 1000
# Bursts against small queues, with maximum IPv4 UDP payloads.
just linux-benchmark --rates 1000,10000 --samples 1000 --burst 32 --capacity 8 --payload-bytes 1472
# Drop every fourth acknowledgement after a synthetic processing delay.
just benchmark --rates 1000 --samples 100 --peer-delay-us 100 --drop-every 4
```

Use the same flags for either recipe. `--backend` belongs only to direct
`cargo run --release --example transport_benchmark -- ...` invocation; the Linux
runner owns that choice. The simulator uses bounded in-process queues. It proves
generator/accounting behavior, not packet parsing or network throughput. The
veth backend uses IPv4 UDP with `Require` checksum policy on both endpoints.

## Workload contract

| Option | Default | Bounds / meaning |
| --- | --- | --- |
| `--rates` | `100,1000,10000` | Strictly increasing list of 1–32 rates, each 1–1,000,000 commands/s |
| `--samples` | `200` | 1–100,000 commands per rate; nominal duration at most 300 s per rate |
| `--payload-bytes` | `64` | 29–1472 bytes, including the synthetic header |
| `--burst` | `1` | 1–samples commands issued together; groups follow the requested average rate |
| `--capacity` | `64` | 1–65,536 packets per receive queue |
| `--telemetry-per-command` | `1` | 0–8 telemetry packets before each acknowledgement |
| `--telemetry-rate` | `0` | 0–1,000,000 independently scheduled telemetry packets/s |
| `--peer-delay-us` | `0` | 0–100,000 us requested processing sleep per received command |
| `--drop-every` | `0` | Zero disables injection; N omits every Nth command acknowledgement |
| `--drain-ms` | `500` | 1–10,000 ms receive grace after issuing commands and the planned background timeline |

Background packet count is `ceil(samples * telemetry_rate / command_rate)`.
Combined telemetry storage is limited to one million observations per trial.
Scheduling uses absolute monotonic targets. When late, the generator catches up;
it does not silently reduce the requested sample count. Actual sleep duration
depends on the OS scheduler. The peer generates background telemetry and handles
commands on one thread, so processing delays can delay background generation.
The report includes both command and background schedule lateness.

The synthetic header is `CEB1`, a one-byte kind, an eight-byte epoch, an eight-byte
sequence, and an eight-byte timestamp, followed by zero padding. Integers use
network byte order. Kinds are command 1, acknowledgement 2, and telemetry 3.
Acknowledgements echo the command identity and host timestamp. Telemetry uses
its generation timestamp. Both threads share a monotonic clock origin. This is
not a CCSDS command encoding and must not be sent to an arbitrary peer.

## JSON schema version 1

Trial records include the workload inputs, backend, OS, architecture, crate
version, and debug-build flag. Record the source commit, kernel, CPU model,
power settings, and competing load with each saved run as external metadata.
Repeat each workload; this tool does not claim statistical confidence from one
sweep and applies no acceptance thresholds.

- `offered`, `sent`, and error counters separate attempted requests from
  successful local sends. `peer_commands` counts commands observed by the peer.
  A successful send does not prove delivery. A frame still queued when the
  receive window ends counts as missing within that window.
- `command_rtt` and `telemetry_age` contain sample count, nearest-rank
  p50/p95/p99, and maximum, in nanoseconds. Empty distributions are `null`, not
  zero. Unique packets only contribute; duplicates, reordered acknowledgements,
  invalid identities, and impossible timestamps are counted separately.
- `missing_acks` is successful sends minus unique observed acknowledgements.
  `intentional_ack_drops` reports the subset deliberately omitted by the peer.
  `missing_telemetry` compares observed unique telemetry to the requested
  background count plus successful sends times telemetry-per-command.
  `peer_telemetry_attempted` helps distinguish ungenerated from unobserved data.
- `issue_span_ns` ends at the last command-send attempt. `measurement_ns` ends
  when receiving finishes. `achieved_offer_rate_per_s` excludes the first burst
  from numerator and timing window; it is `null` for a single burst. Inspect it
  and `max_schedule_lateness_ns` before treating a requested rate as achieved.
- `host_queue` and `peer_queue` contain capacity, high-water mark, user-space
  drops, kernel drops, and statistics failures. Simulator drops occur at its
  bounded incoming queue. Veth counters describe the real transport ring and
  kernel observations; these scopes are not interchangeable.
- `cpu_process_ns` is Linux process CPU time for both threads; it is `null` on
  unsupported platforms. `allocation_calls` counts successful allocation,
  zeroed-allocation, and reallocation calls. `allocation_requested_bytes` sums
  requested sizes, not live memory or net growth. Instrumentation is atomic and
  process-wide, and includes the generator, peer, transport, and measurement
  overhead. It is not library-only or Python cost. Buffers and endpoints are
  prepared before snapshots; snapshots span worker release through joined
  shutdown. `accounting_ns` records that wall-clock interval. Peer polling can
  add idle shutdown time. Peer close is included. Formatting, host close, and
  reopen costs are excluded.
- `reopen_to_echo_ns` measures opening a fresh pair, starting receive, and a
  fresh synchronous echo after closing the measured pair. It is `null` if the
  probe fails, with a diagnostic on stderr. It is not link-outage detection,
  remote boot time, or reconciliation of a prior command.

The summary reports the highest tested requested rate without observed packet
loss/errors and the first tested requested rate with loss/errors. These are
sampled observations, **not** a discovered throughput limit. Outcomes need not
be monotonic. Fault-injected sweeps are flagged. Short trials, scheduler lag,
unobserved kernel drops, and generator limits can all invalidate capacity claims.

## Verification and later physical tests

`just check` exercises parsing, accounting, JSON, independent telemetry, empty
distributions, and scripted ACK loss without privileges. `just linux-benchmark-check`
also exercises the same JSON contract through the disposable veth fixture.

Later physical tests need explicit consumer-owned addresses and a peer adapter
for that protocol. Preserve request identity, no-retry semantics, and factual
measurements. Do not assume a remote clock shares this process origin: use echoed
host timestamps for RTT and qualify clock alignment before reporting remote
telemetry age. Keep device configuration and results outside public source.

Use measured workloads to evaluate filtering, reuse, statistics sampling, and
batching separately. Packet mmap remains deferred until simpler measurements
show a need. No optimization or physical-peer compatibility conclusion follows
from a synthetic benchmark alone.
