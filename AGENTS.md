# Repository instructions

- Development and live transport use are Linux-led. Run `just check` for validation.
- GitHub Actions are allowed for this public repository. Never install or invoke TruffleHog in this repository.
- Releases use `vYYYY.MM.DD.XX`. For ordinary release execution, use the `cut-release` skill and the checked-in `just cut-release` runner.
- Generic shared implementation must not define consumer intent or safety policy.
