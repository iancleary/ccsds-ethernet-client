# Frame parser fuzz target

Run `cargo +nightly fuzz run frames -- -max_len=2048 -max_total_time=60`
from the repository root after installing cargo-fuzz. Preserve minimized
failures as ordinary regression tests. The target exercises arbitrary IPv4
and IPv6 frames without opening sockets. Normal tests also run deterministic
arbitrary-input and truncation coverage.
