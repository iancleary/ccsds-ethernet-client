# Dependency inventory

The default Rust library and tests use only the Rust standard library. Linux
builds use the locked `libc` crate for the isolated `AF_PACKET` system-call
boundary. The optional `python` feature uses the locked PyO3 dependency and
its procedural-macro dependencies. Live raw-socket access requires
deployment-managed `CAP_NET_RAW`; that capability is not needed by the
hardware-free test suite. `Cargo.lock` is the authoritative resolved-version
inventory and remains checked in.

The Python extension is built with Maturin 1.11.5. The standalone release
runner and release workflow tests use only the Python standard library and run
with `uv`; local release execution also uses `git`, `just`, and the GitHub CLI.
These tools are not Rust runtime dependencies. No async runtime, pcap/libpcap,
serialization, CLI, RPC, service, database, or recording dependency is
included.
