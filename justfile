set shell := ["bash", "-euo", "pipefail", "-c"]

# Show the available recipes.
default:
    @just --list

# Run deterministic, hardware-free tests.
test:
    cargo test --locked

# Build the extension and run hardware-free Python contract tests.
python-test:
    uv run --python 3.11 --isolated --no-project --with maturin==1.11.5 bash -euc 'maturin develop --locked; python -m unittest discover -s python/tests -v'

# Format, test, lint, and check patch whitespace.
check:
    cargo fmt --check
    cargo test --locked
    cargo clippy --locked --all-targets -- -D warnings
    PYO3_NO_PYTHON=1 cargo clippy --locked --features python --all-targets -- -D warnings
    just python-test
    uv run scripts/test_cut_release.py
    uv run scripts/test_release_workflow.py
    git diff --check -- .

# Validate and create a SemVer GitHub release.
[positional-arguments]
cut-release *args:
    uv run scripts/cut_release.py "$@"
