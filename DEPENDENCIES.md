# Dependency inventory

The portable library and tests use only the Rust standard library. Linux
builds use the locked `libc` crate for the isolated `AF_PACKET` system-call
boundary. Live raw-socket access requires deployment-managed `CAP_NET_RAW`;
that capability is not needed by the hardware-free test suite. `Cargo.lock` is
the authoritative resolved-version inventory and remains checked in.

The standalone release runner is Python-standard-library-only and is executed
with `uv`; local release execution also uses `git`, `just`, and the GitHub CLI.
These tools are not Rust package dependencies. No async runtime, pcap/libpcap,
serialization, CLI, RPC, service, database, recording dependency, GitHub
Actions workflow, or registry publishing dependency is included.
