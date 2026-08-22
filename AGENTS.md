# Repository instructions

- Development and live transport use are Linux-led. Run `just check` for validation.
- Do not add GitHub Actions. Never install or invoke TruffleHog in this repository.
- Releases use `vYYYY.MM.DD.XX`. For ordinary release execution, use the `cut-release` skill and the checked-in `just cut-release` runner.
- Generic shared implementation must not define subsystem intent or safety policy.
