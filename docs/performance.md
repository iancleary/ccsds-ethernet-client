# Performance evidence and next measurements

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

The full workload benchmark remains open. Measure telemetry age, offered-rate
loss thresholds, queue high-water marks, CPU time, allocations per packet, and
recovery time. Cover idle periods, steady traffic, bursts, maximum payloads,
unrelated traffic, peer resets, and interface replacement. Use externally
defined workloads and acceptance limits. No physical-link measurement or
peer-specific checksum compatibility result is claimed here.

## Optimization gates

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
