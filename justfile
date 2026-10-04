set shell := ["bash", "-euo", "pipefail", "-c"]

# Show the available recipes.
default: help

# Show the available recipes.
help:
    @just --list

# Format the Rust code.
fmt:
    cargo fmt --all

# Alias for fmt.
format: fmt

# Check formatting without writing changes.
fmt-check:
    cargo fmt --all -- --check

# Lint the default crate and optional Python binding without writing changes.
lint:
    cargo clippy --locked --all-targets -- -D warnings
    PYO3_NO_PYTHON=1 cargo clippy --locked --features python --all-targets -- -D warnings

# Check documentation with rustdoc warnings denied.
doc-check:
    PYO3_NO_PYTHON=1 RUSTDOCFLAGS="-D warnings" cargo doc --locked --all-features --no-deps

# Verify the Rust package without publishing.
package:
    cargo package --locked

# Build the Rust crate for release.
build:
    cargo build --locked --release

# Run deterministic, hardware-free tests.
test:
    cargo test --locked --all-targets
    cargo test --locked --doc

# Requires Linux, iproute2, and permission to create disposable network namespaces.
[positional-arguments]
linux-test *args:
    uv run --managed-python --python 3.11 --isolated --no-project scripts/test_linux_live.py "$@"

# Root-free workload simulation; JSON Lines go to stdout.
[positional-arguments]
benchmark *args:
    cargo run --quiet --locked --release --example transport_benchmark -- "$@"

# Actual packet transport in a disposable Linux veth fixture.
[positional-arguments]
linux-benchmark *args:
    uv run --managed-python --python 3.11 --isolated --no-project scripts/test_linux_live.py --release --benchmark "$@"

# Verify benchmark accounting with simulation and the isolated packet transport.
linux-benchmark-check:
    CCSDS_BENCHMARK_VETH=1 uv run --managed-python --python 3.11 scripts/test_transport_benchmark.py

# Build the extension and run hardware-free Python contract tests.
python-test:
    uv run --managed-python --python 3.11 --isolated --no-project --with maturin==1.11.5 bash -euc 'maturin develop --locked; python -m unittest discover -s python/tests -v'

# Verify release tooling, benchmark accounting, and patch whitespace.
policy-check:
    uv run --managed-python --python 3.11 scripts/test_cut_release.py
    uv run --managed-python --python 3.11 scripts/test_release_workflow.py
    uv run --managed-python --python 3.11 scripts/test_transport_benchmark.py
    uv run --managed-python --python 3.11 scripts/test_task_runtime.py
    git diff --check -- .

# Run all hardware-free validation, including the Python contract.
check: fmt-check lint test doc-check python-test policy-check package

# Run the complete validation gate and release build.
ci: check build

# Validate and create a SemVer GitHub release.
[positional-arguments]
cut-release *args:
    uv run --managed-python --python 3.11 scripts/cut_release.py "$@"
