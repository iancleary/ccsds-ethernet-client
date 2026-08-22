set shell := ["bash", "-euo", "pipefail", "-c"]

# Show the available recipes.
default:
    @just --list

# Run deterministic, hardware-free tests.
test:
    cargo test --locked

# Format, test, lint, and check patch whitespace.
check:
    cargo fmt --check
    cargo test --locked
    cargo clippy --locked --all-targets -- -D warnings
    uv run scripts/test_cut_release.py
    git diff --check -- .

# Validate and create a SemVer GitHub release.
[positional-arguments]
cut-release *args:
    uv run scripts/cut_release.py "$@"
