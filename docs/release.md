# Release process

Repository releases use ASCII UTC-date CalVer tags of the form
`vYYYY.MM.DD.XX`, where `XX` starts at `00` each day and increments. The Rust
crate version is independent SemVer and may remain `0.1.0`; the package is
`publish = false` and this process never publishes to a registry or edits
`Cargo.toml`/`Cargo.lock`. There are no GitHub Actions in this process.

Use the `cut-release` skill for ordinary execution. The checked-in runner is
invoked through `just`:

```sh
just cut-release --print-current-version
just cut-release --print-next-version
just cut-release --version v2026.08.18.00 --notes-file /path/to/notes.md --dry-run
just cut-release --version v2026.08.18.00 --notes-file /path/to/notes.md
```

The two print commands are read-only: they verify the canonical `origin`, then
inspect local and remote tags without fetching or modifying refs. With no
release tags, current prints `none`; next prints sequence `00` for the current
UTC date. Release execution always requires an explicit version and a
non-empty notes file. Notes may be kept outside the repository. The runner
reads them once before validation and sends that reviewed snapshot to `gh`
through standard input rather than reopening the path.

## Preconditions and ordering

Run on clean `main` with `git`, `gh`, `just`, and `uv` installed and GitHub CLI
authenticated. `origin` must resolve directly to
`iancleary/ccsds-ethernet-client` on `github.com`; lookalike hosts and
other owners or repositories are rejected. Every `gh` operation explicitly
selects `github.com` and that repository.

The runner:

1. validates the ASCII CalVer syntax, real UTC date, notes, clean tree, branch,
   and canonical origin;
2. fetches `origin/main` and tags, requires `HEAD` to equal `origin/main`, and
   requires the requested version to be the next daily sequence;
3. verifies GitHub CLI authentication and runs `just check`;
4. requires the tree to remain clean, then verifies the canonical origin
   again, fetches again, and rechecks both the validated `HEAD` and the next
   version against the latest `origin/main` and tags;
5. inspects the exact remote tag SHA and GitHub Release state, rejecting an
   existing release, a non-404 API failure, or a tag at any SHA other than the
   validated `HEAD`;
6. atomically creates `refs/tags/<version>` at the validated SHA with a direct
   tag push when the tag is absent; and
7. as the last action, creates the GitHub Release with `--verify-tag` and the
   reviewed notes snapshot from standard input.

The final two remote writes for a new release are the tag push and then GitHub
Release creation. There is no branch push, commit, manifest mutation, registry
publish, or workflow invocation. `--dry-run` performs every preflight and
repository check, prints both prospective write commands, and executes neither
write. Fetching remote refs and normal build outputs are its only local
effects.

## Recovery after the final commands

GitHub Release creation intentionally remains last. If the tag push succeeds
but Release creation fails, do not immediately rerun, force-push, or delete the
tag. First inspect both states:

```sh
git ls-remote --tags --refs origin refs/tags/vYYYY.MM.DD.XX
gh api --hostname github.com \
  repos/iancleary/ccsds-ethernet-client/releases/tags/vYYYY.MM.DD.XX
```

Compare the remote tag SHA to the validated `main` SHA. If the tag is exactly
at that SHA and the release query is an HTTP 404, rerunning the same
`just cut-release` command safely skips the tag push and retries Release
creation. A tag at another SHA is an integrity failure and the runner rejects
it. If the Release exists, stop; the release completed even if the local
command result was ambiguous. For authentication, transport, or other API
errors, resolve the error and inspect both states again before retrying or
deleting anything.
