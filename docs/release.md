# Release process

Repository releases use ASCII SemVer tags of the form `vMAJOR.MINOR.PATCH`.
The tag, Rust crate, and Python package use the version in `Cargo.toml`.
Publishing the GitHub Release triggers `.github/workflows/release.yml`. The
workflow builds all artifacts, publishes the crate to crates.io, and then
publishes the Python distributions to PyPI. The local runner does not edit
`Cargo.toml`, `Cargo.lock`, or `pyproject.toml`.

Use the `cut-release` skill for ordinary execution. The checked-in runner is
invoked through `just`:

```sh
just cut-release --print-current-version
just cut-release --print-next-version
just cut-release --version v0.1.1 --notes-file /path/to/notes.md --dry-run
just cut-release --version v0.1.1 --notes-file /path/to/notes.md
```

The two print commands are read-only: they verify the canonical `origin`, then
inspect local and remote tags without fetching or modifying refs. Current
prints the highest released SemVer tag or `none`. Next prints the version from
`Cargo.toml` when that version is not released. It fails when the manifest
version is already tagged, so the manifest must be bumped before another
release. Release execution always requires an explicit version and a non-empty
notes file. Notes may be kept outside the repository. The runner reads them
once before validation and sends that reviewed snapshot to `gh` through
standard input rather than reopening the path.

## Preconditions and ordering

Run on clean `main` with `git`, `gh`, `just`, and `uv` installed and GitHub CLI
authenticated. `origin` must resolve directly to
`iancleary/ccsds-ethernet-client` on `github.com`; lookalike hosts and
other owners or repositories are rejected. Every `gh` operation explicitly
selects `github.com` and that repository.

The runner:

1. validates the ASCII SemVer syntax, manifest version, notes, clean tree,
   branch, and canonical origin;
2. fetches `origin/main` and tags, requires `HEAD` to equal `origin/main`, and
   requires the requested version to match `Cargo.toml`;
3. verifies GitHub CLI authentication and runs `just check`;
4. requires the tree to remain clean, then verifies the canonical origin
   again, fetches again, and rechecks both the validated `HEAD` and the package
   version against the latest `origin/main`;
5. inspects the exact remote tag SHA and GitHub Release state, rejecting an
   existing release, a non-404 API failure, or a tag at any SHA other than the
   validated `HEAD`;
6. atomically creates `refs/tags/<version>` at the validated SHA with a direct
   tag push when the tag is absent; and
7. as the last local action, creates the GitHub Release with `--verify-tag` and
   the reviewed notes snapshot from standard input.

The final two direct remote writes for a new release are the tag push and then
GitHub Release creation. Release creation triggers the package workflow. The
local runner does not push a branch, create a commit, mutate a manifest, or
invoke a workflow directly. `--dry-run` performs every preflight and repository
check, prints both prospective write commands, and executes neither write.
Fetching remote refs and normal build outputs are its only local effects.

## Registry workflow

Before the first release, configure these repository and registry settings:

- Add a GitHub `crates-io` environment. Store a crates.io publish token in the
  repository secret `CARGO_REGISTRY_TOKEN`. The token is available only to the
  crate publish step.
- Add a GitHub `pypi` environment. Configure the PyPI project
  `ccsds-ethernet-client` to trust repository
  `iancleary/ccsds-ethernet-client`, workflow `release.yml`, and environment
  `pypi`. The workflow uses OIDC Trusted Publishing and stores no PyPI token.
- Add required reviewers or tag protection to the two environments when the
  repository policy requires manual publication approval.

The workflow performs this sequence:

1. Verify that the GitHub Release tag matches `Cargo.toml`. Run the Rust,
   Python, release-runner, and workflow contract checks.
2. Build CPython 3.11 stable-ABI wheels for Linux x86-64 and AArch64. Build a
   source distribution. Store each output as a workflow artifact.
3. After every build succeeds, publish the crate with `cargo publish --locked`.
4. After crates.io publication succeeds, download the previously built Python
   artifacts and publish them through the `pypi` environment.

The PyPI job alone receives `id-token: write`. Build jobs have read-only
repository permission and no registry credentials. Third-party actions and
Maturin are pinned. Update the pins in a reviewed pull request.

If `cargo publish` reports an ambiguous timeout or index error, inspect the
requested version on crates.io before rerunning the publish job. Do not assume
that a failed job means the upload failed.

If crates.io publication succeeds and PyPI publication fails, do not rerun the
whole GitHub Release event blindly. Inspect the release workflow and PyPI
project first. The crate version is permanent. Re-run only the failed PyPI job
after fixing the environment or Trusted Publisher configuration. The stored
artifacts from that workflow run remain the release inputs.

## Recovery after the final commands

GitHub Release creation intentionally remains last. If the tag push succeeds
but Release creation fails, do not immediately rerun, force-push, or delete the
tag. First inspect both states:

```sh
git ls-remote --tags --refs origin refs/tags/vMAJOR.MINOR.PATCH
gh api --hostname github.com \
  repos/iancleary/ccsds-ethernet-client/releases/tags/vMAJOR.MINOR.PATCH
```

Compare the remote tag SHA to the validated `main` SHA. If the tag is exactly
at that SHA and the release query is an HTTP 404, rerunning the same
`just cut-release` command safely skips the tag push and retries Release
creation. A tag at another SHA is an integrity failure and the runner rejects
it. If the Release exists, stop; the release completed even if the local
command result was ambiguous. For authentication, transport, or other API
errors, resolve the error and inspect both states again before retrying or
deleting anything.
