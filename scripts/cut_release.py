#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Validate and create a CalVer GitHub release without publishing the crate."""

from __future__ import annotations

import argparse
import datetime as dt
import re
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HOST = "github.com"
REPOSITORY = "iancleary/ccsds-ethernet-client"
GH_REPOSITORY = f"{HOST}/{REPOSITORY}"
VERSION_RE = re.compile(r"v([0-9]{4})\.([0-9]{2})\.([0-9]{2})\.([0-9]{2})", re.ASCII)
ORIGIN_RE = re.compile(
    rf"(?:git@{re.escape(HOST)}:|https://{re.escape(HOST)}/|ssh://git@{re.escape(HOST)}/)"
    rf"{re.escape(REPOSITORY)}(?:\.git)?",
    re.ASCII,
)


def run(
    *command: str,
    capture: bool = False,
    check: bool = True,
    input_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=ROOT,
        check=check,
        text=True,
        capture_output=capture,
        input=input_text,
    )


def parse_version(version: str) -> tuple[dt.date, int]:
    match = VERSION_RE.fullmatch(version)
    if not match:
        raise ValueError("version must match ASCII vYYYY.MM.DD.XX")
    year, month, day, sequence = map(int, match.groups())
    return dt.date(year, month, day), sequence


def verify_origin() -> None:
    urls = []
    for command in (
        ("git", "remote", "get-url", "--all", "origin"),
        ("git", "remote", "get-url", "--push", "--all", "origin"),
    ):
        urls.extend(run(*command, capture=True).stdout.splitlines())
    if not urls or any(not ORIGIN_RE.fullmatch(url) for url in urls):
        raise RuntimeError(
            f"origin fetch and push URLs must be the canonical {REPOSITORY} repository on {HOST}"
        )


def release_versions() -> list[str]:
    verify_origin()
    local = run("git", "tag", "--list", "v*", capture=True).stdout.splitlines()
    remote = run(
        "git", "ls-remote", "--tags", "--refs", "origin", "refs/tags/v*", capture=True
    ).stdout.splitlines()
    versions = set(local)
    versions.update(line.rsplit("/", 1)[-1] for line in remote)
    return sorted((version for version in versions if VERSION_RE.fullmatch(version)), key=parse_version)


def remote_tag_sha(version: str) -> str | None:
    output = run(
        "git",
        "ls-remote",
        "--tags",
        "--refs",
        "origin",
        f"refs/tags/{version}",
        capture=True,
    ).stdout.splitlines()
    if not output:
        return None
    if len(output) != 1:
        raise RuntimeError(f"could not uniquely resolve remote tag: {version}")
    sha, ref = output[0].split(maxsplit=1)
    if ref != f"refs/tags/{version}":
        raise RuntimeError(f"remote returned an unexpected tag ref: {ref}")
    return sha


def next_version(versions: list[str], today: dt.date) -> str:
    sequences = []
    for version in versions:
        release_date, sequence = parse_version(version)
        if release_date == today:
            sequences.append(sequence)
    sequence = max(sequences, default=-1) + 1
    if sequence > 99:
        raise ValueError(f"no release sequence remains for {today.isoformat()}")
    return f"v{today:%Y.%m.%d}.{sequence:02d}"


def validate_next_version(version: str, versions: list[str], today: dt.date) -> None:
    expected = next_version([candidate for candidate in versions if candidate != version], today)
    if version != expected:
        raise ValueError(f"version must be the next release ({expected})")


def verify_release_absent(version: str) -> None:
    existing = run(
        "gh",
        "api",
        "--hostname",
        HOST,
        f"repos/{REPOSITORY}/releases/tags/{version}",
        capture=True,
        check=False,
    )
    if existing.returncode == 0:
        raise RuntimeError(f"GitHub release already exists: {version}")
    if "HTTP 404" not in existing.stderr:
        raise RuntimeError(f"could not verify GitHub release absence: {existing.stderr.strip()}")


def verify_head(head: str | None = None) -> str:
    current = run("git", "rev-parse", "HEAD", capture=True).stdout.strip()
    origin_main = run("git", "rev-parse", "refs/remotes/origin/main", capture=True).stdout.strip()
    if current != origin_main:
        raise RuntimeError("main must exactly match origin/main")
    if head is not None and current != head:
        raise RuntimeError("HEAD changed while repository checks ran")
    return current


def release(version: str, notes_file: Path, dry_run: bool, today: dt.date | None = None) -> None:
    today = today or dt.datetime.now(dt.timezone.utc).date()
    release_date, _ = parse_version(version)
    if release_date != today:
        raise ValueError(f"release date must be today's UTC date ({today.isoformat()})")
    if not notes_file.is_file():
        raise ValueError("notes file must exist and be non-empty")
    notes = notes_file.read_text(encoding="utf-8")
    if not notes.strip():
        raise ValueError("notes file must exist and be non-empty")
    for tool in ("git", "gh", "just"):
        if not shutil.which(tool):
            raise RuntimeError(f"required tool not found: {tool}")

    if run("git", "status", "--porcelain", capture=True).stdout:
        raise RuntimeError("working tree must be clean")
    if run("git", "branch", "--show-current", capture=True).stdout.strip() != "main":
        raise RuntimeError("release must run from main")
    verify_origin()

    run("git", "fetch", "--quiet", "origin", "main", "--tags")
    head = verify_head()
    validate_next_version(version, release_versions(), today)

    run("gh", "auth", "status", "--hostname", HOST)
    run("just", "check")
    if run("git", "status", "--porcelain", capture=True).stdout:
        raise RuntimeError("repository checks changed the working tree")

    verify_origin()
    run("git", "fetch", "--quiet", "origin", "main", "--tags")
    head = verify_head(head)
    validate_next_version(version, release_versions(), today)

    tag_sha = remote_tag_sha(version)
    verify_release_absent(version)
    if tag_sha is not None and tag_sha != head:
        raise RuntimeError(f"remote tag {version} exists at {tag_sha}, expected {head}")

    push_command = ("git", "push", "origin", f"{head}:refs/tags/{version}")
    release_command = (
        "gh",
        "release",
        "create",
        version,
        "--repo",
        GH_REPOSITORY,
        "--title",
        version,
        "--notes-file",
        "-",
        "--verify-tag",
    )
    if dry_run:
        print("DRY RUN: " + shlex.join(push_command))
        print("DRY RUN: " + shlex.join(release_command))
        return
    if tag_sha is None:
        run(*push_command)
    run(*release_command, input_text=notes)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    query = parser.add_mutually_exclusive_group()
    query.add_argument("--print-current-version", action="store_true")
    query.add_argument("--print-next-version", action="store_true")
    parser.add_argument("--version")
    parser.add_argument("--notes-file", type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)

    try:
        if args.print_current_version:
            versions = release_versions()
            print(versions[-1] if versions else "none")
            return 0
        if args.print_next_version:
            today = dt.datetime.now(dt.timezone.utc).date()
            print(next_version(release_versions(), today))
            return 0
        if not args.version or not args.notes_file:
            parser.error("release execution requires --version and --notes-file")
        release(args.version, args.notes_file.resolve(), args.dry_run)
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
