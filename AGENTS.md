# Repository instructions

- Development and live transport use are Linux-led. Run `just check` for validation.
- GitHub Actions are allowed for this public repository. Never install or invoke TruffleHog in this repository.
- Rust and Python releases share the `Cargo.toml` version and SemVer tags of the form `vMAJOR.MINOR.PATCH`. For ordinary release execution, use the `cut-release` skill and the checked-in `just cut-release` runner. GitHub Actions builds all artifacts, publishes crates.io first, and then publishes PyPI.
- Generic shared implementation must not define consumer intent or safety policy.
- Before changing behavior, read `docs/agent-operating-loop.md` and preserve its invariants unless the PR explicitly changes the public contract.
- Accrete future agent knowledge into the nearest executable test or durable doc. Avoid one-off plans that do not update `README.md`, `docs/`, `AGENTS.md`, or the release runner.
