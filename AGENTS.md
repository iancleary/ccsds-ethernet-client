# Repository instructions

- Development and live transport use are Linux-led. Run `just check` for validation.
- GitHub Actions are allowed for this public repository. Never install or invoke TruffleHog in this repository.
- Releases use SemVer tags of the form `vMAJOR.MINOR.PATCH` and must match `Cargo.toml`. For ordinary release execution, use the `cut-release` skill and the checked-in `just cut-release` runner.
- Generic shared implementation must not define consumer intent or safety policy.
- Before changing behavior, read `docs/agent-operating-loop.md` and preserve its invariants unless the PR explicitly changes the public contract.
- Accrete future agent knowledge into the nearest executable test or durable doc. Avoid one-off plans that do not update `README.md`, `docs/`, `AGENTS.md`, or the release runner.
