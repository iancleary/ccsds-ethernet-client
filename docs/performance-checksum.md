# Checksum allocation evaluation

The v0.3.2 checksum implementation sums the pseudo-header fields and UDP slice
without allocating or copying a temporary concatenation buffer. Partial sums
follow [RFC 1071 section 2(A)](https://www.rfc-editor.org/rfc/rfc1071#section-2):
all boundaries preceding the UDP slice contain an even number of bytes. Only
the last UDP byte can need zero padding. Generation still transmits a computed
zero checksum as `0xffff`; validation still checks the unmodified received sum.

The receive frame scratch buffer already lives on the stack and is reused within
a drain pass. Returned payloads have owned storage. This change removes checksum
scratch allocations without changing that ownership or the public API.

## Controlled virtual-link comparison

Compare v0.3.1 (`cc15ebea3a61588a23c28012800d9a5b9d41df05`) with the
checksum change in this release. Both binaries used Rust 1.98.0, release profile,
the same lockfile, and native AArch64 Linux in a disposable container. The kernel
was `7.0.14-orbstack-00380-ga7e0a2dc9535`. Both endpoints shared a process and clock.
The container used a private network namespace and veth pair. CPU affinity,
power settings, and competing load were not controlled; timing results therefore
have limited scope. No physical peer was used.

For each workload, run five trials per binary. Alternate baseline/candidate order
between repetitions. Use 1,000 commands, one telemetry packet per command, no
background traffic, no injected delay/loss, and the default 500 ms drain grace.
Both endpoints require IPv4 checksums. The rows below specify the changed flags:

| Workload | Requested commands/s | Payload bytes | Burst | Capacity |
| --- | ---: | ---: | ---: | ---: |
| Steady | 1,000 | 256 | 1 | 64 |
| Maximum payload | 10,000 | 1472 | 1 | 64 |
| Burst | 10,000 | 256 | 16 | 64 |
| Queue pressure | 10,000 | 1472 | 32 | 8 |

Use `just linux-benchmark --rates RATE --samples 1000 --payload-bytes BYTES
--burst BURST --capacity CAPACITY` in separate baseline and candidate checkouts
to repeat these workloads. Preserve every JSON trial and its summary. Compare
achieved rates, pacing lateness, loss, and counters before interpreting CPU time.

Every capacity-64 trial observed all 1,000 ACKs and 1,000 telemetry packets.
Allocation counts were identical across repeats of each version:

| Payload | Allocation calls, baseline → candidate | Requested bytes, baseline → candidate |
| --- | ---: | ---: |
| 256 | 15,000 → 9,000 | 4,086,000 → 2,430,000 |
| 1472 | 15,000 → 9,000 | 22,326,000 → 13,374,000 |

Three datagrams per command each undergo generation and validation. Removing
one checksum allocation at each step accounts for 6,000 fewer calls, a 40%
reduction in measured process-wide calls for these complete workloads. This
percentage includes generator and peer overhead and is not a library-only metric.

Ranges across the five trials show why timing claims must remain limited:

| Workload | Process CPU ms, baseline → candidate | RTT p99 us, baseline → candidate |
| --- | --- | --- |
| Steady | 36.5–46.6 → 35.4–41.2 | 154.6–376.8 → 134.9–209.8 |
| Maximum payload | 41.7–47.8 → 41.5–47.1 | 1879.1–2221.2 → 1814.5–2166.5 |
| Burst | 20.5–24.9 → 20.6–23.4 | 912.8–1258.4 → 917.7–1014.9 |
| Queue pressure | 42.3–49.2 → 40.2–46.0 | 1958.2–2598.1 → 2193.6–3072.3 |

Queue-pressure trials lost 6–90 ACKs on the baseline and 12–96 on the candidate.
Host kernel drops ranged from 12–180 and 24–192 respectively; peer kernel and
user-space ring drops were zero. This does not show a reliable loss improvement.
Requested rates were not achieved exactly. Maximum command schedule lateness
was several milliseconds, so these short trials do not locate a throughput limit.

## Decisions and remaining evidence

Accept removal of the checksum scratch buffers: allocation savings are exact,
and the independent checksum oracle and packet-socket policy matrix preserve the
wire behavior. No CPU, latency, or capacity improvement is claimed.

Keep kernel filtering deferred. The current observations include outgoing,
foreign, VLAN, and malformed frames. A filter could suppress those counters.
First capture a representative unrelated-traffic workload and define the scope
of counters with filtering enabled. A filter must be an explicit contract choice.

Keep statistics sampling unchanged. Reading PACKET_STATISTICS after drain passes
and at close accumulates reset-on-read kernel counters. A sampling interval needs
an explicit freshness contract and a measured benefit before changing it.

Keep recvmmsg batching deferred. The current loop stops at the ring capacity,
checks the deadline per packet, and has a packet budget. A batch must fit available
storage and preserve metadata, truncation handling, ordering, and those bounds.
Queue-pressure loss is a useful stress signal, but no representative workload or
acceptance limit establishes that a more complex receive path is needed yet.
Compare any candidate on loss, tail latency, and CPU, including unrelated traffic.

Keep packet mmap deferred. Its prerequisite, evidence that simpler socket
improvements are insufficient, remains unmet. Revisit only with a measured
requirement. Before implementation, define mapped-ring ownership, frame lifetime,
truncation behavior, supported kernels, and explicit fallback behavior.

Physical-peer checksum qualification and workload acceptance remain open. Use the
[qualification procedure](checksum-qualification.md) and a consumer-owned peer
adapter before extending these virtual-link observations to a deployment.
