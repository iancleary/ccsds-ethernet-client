# Performance evidence and next measurements

The configurable generator and synthetic peer are documented in
[Transport workload benchmark](benchmark.md). Use `just benchmark` for root-free
simulation and `just linux-benchmark` for the actual transport on disposable
virtual interfaces. They report JSON Lines with load, latency, telemetry age,
loss, queues, process CPU/allocation counts, and explicit reopen timing.

Run the virtual-link baseline on Linux:

```sh
just linux-test --release
```

The fixture creates a disposable network namespace and veth pair. The benchmark
uses 1,000 four-byte exchanges on one thread with ring capacity eight. It prints
JSON with sample count, wall time, p50/p95/p99/max exchange latency, and observed
kernel drops. Both endpoints run in the test process. This is not physical-link
round-trip time, scheduler isolation, or peer processing time.

An AArch64 Linux container smoke run of the debug build measured p50 13.5 us,
p95 16.2 us, and p99 20.0 us, with zero observed kernel drops. This single run
only proves the measurement path works. Do not use it as an acceptance limit or
compare it to an optimized build. Keep the build profile, kernel, CPU, load,
payload size, checksum policy, queue sizes, and complete output with future runs.

The workload harness is available; representative physical-link qualification
remains open. Define workloads and acceptance limits before interpreting its
sampled loss observations. Cover idle periods, steady traffic, bursts, maximum
payloads, unrelated traffic, peer resets, and interface replacement with the
appropriate peer adapter. No physical-link measurement or peer-specific checksum
compatibility result is claimed here.

## Optimization gates

The current evaluation selects the existing receive path. Kernel filtering,
statistics sampling changes, recvmmsg batching, and packet mmap are not selected
for implementation. Reopen each decision when the evidence described below is
available. These decisions do not block independently verified buffer reuse.

The first allocation comparison is recorded in
[Checksum allocation evaluation](performance-checksum.md). It supports removing
temporary checksum buffers. CPU and latency ranges overlap, and it does not
establish a throughput improvement or a physical-link acceptance limit.

Do not enable kernel filtering by default yet. Filtering can remove frames
before observation counters see them. Define counter scope and compare the
same workloads with and without the filter before changing that contract.

Do not add recvmmsg batching yet. The current drain loop stops when its bounded
ring is full and honors the deadline and packet budget. Further batching must
preserve those rules. Compare packet loss, tail latency, and CPU use before
accepting throughput gains. Measure buffer reuse and statistics-read frequency
separately so each change has attributable evidence.

Do not add packet mmap yet. There is no representative evidence that ordinary
socket receive is insufficient. Revisit it only after the simpler measurements.
Any proposal must define ring ownership, buffer lifetime, truncation, kernel
compatibility, and fallback behavior before implementation.

## Transmit buffer reuse

The v0.3.3 Linux transport reserves 2 KiB of transmit storage when it opens and
reuses that storage for sends. Frame construction clears the visible length
before validation and initializes every transmitted byte, including padding.
Rejected input cannot send the previous frame. The public allocating frame
builder retains its API. Packet bytes, checksum policy, send count, receive
behavior, and returned payload ownership are unchanged.

Compare v0.3.2 (`3700f3cf3d76de16f80f876ee66bf7945266458e`) with this change
using the four workloads and five alternating trials per version in the
[checksum evaluation](performance-checksum.md). Both binaries used Rust 1.98.0,
release builds, and the same native AArch64 Linux veth environment. Affinity,
power settings, and competing load were not controlled. Timing results are
observations only; allocation counts are the acceptance evidence.

All steady and burst trials with 256-byte payloads received all ACKs and
telemetry. For 1,000 commands, measured process-wide allocation calls decreased
from 9,000 to 6,000, and requested bytes from 2,430,000 to 1,536,000, in every
repeat. This removes one frame allocation for each of the 3,000 datagram sends.
Endpoint storage is allocated before the benchmark accounting interval.

Steady-workload CPU ranges were 44.9–52.1 ms before and 36.7–49.6 ms after;
RTT p99 ranges were 177.6–322.9 us before and 179.6–207.6 us after. Burst CPU
ranges were 19.8–24.6 ms before and 21.7–23.3 ms after; RTT p99 ranges were
1065.5–1461.5 us before and 920.0–1190.7 us after. These overlapping ranges do
not establish a general CPU or latency improvement.

Maximum-payload trials lost 10–25 ACKs before and 5–18 after; queue-pressure
trials lost 56–104 before and 25–100 after. Both versions showed scheduling lag.
These short runs do not qualify a capacity limit or eliminate the receive-path
evidence gates above.

Tests compare reused and fresh frames across both IP families and every IPv4
checksum policy, alternating maximum and short payloads. They check allocation
reuse, rejected input, subsequent valid input, and initialized padding. Linux
packet-socket tests verify successive sends and recovery after oversized input.
