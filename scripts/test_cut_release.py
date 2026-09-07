#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///

import contextlib
import io
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import cut_release  # noqa: E402

# Exercise the current release version without maintaining a second version source.
VERSION = cut_release.package_version()
SHA = "a" * 40
CANONICAL_ORIGIN = "git@github.com:iancleary/ccsds-ethernet-client.git"


class FakeRun:
    def __init__(self) -> None:
        self.calls: list[tuple[str, ...]] = []
        self.inputs: list[str | None] = []
        self.statuses = ["", ""]
        self.branch = "main\n"
        self.origin = CANONICAL_ORIGIN + "\n"
        self.push_origin = CANONICAL_ORIGIN + "\n"
        self.heads = [SHA + "\n", SHA + "\n"]
        self.origin_heads = [SHA + "\n", SHA + "\n"]
        self.local_tags: list[str] = []
        self.remote_tags: dict[str, str] = {}
        self.release_returncode = 1
        self.release_stderr = "gh: Not Found (HTTP 404)\n"
        self.on_check = None

    def __call__(
        self,
        *command: str,
        capture: bool = False,
        check: bool = True,
        input_text: str | None = None,
    ) -> subprocess.CompletedProcess[str]:
        self.calls.append(command)
        self.inputs.append(input_text)
        stdout = ""
        stderr = ""
        returncode = 0
        if command == ("git", "status", "--porcelain"):
            stdout = self.statuses.pop(0)
        elif command == ("git", "branch", "--show-current"):
            stdout = self.branch
        elif command == ("git", "remote", "get-url", "--all", "origin"):
            stdout = self.origin
        elif command == ("git", "remote", "get-url", "--push", "--all", "origin"):
            stdout = self.push_origin
        elif command == ("git", "rev-parse", "HEAD"):
            stdout = self.heads.pop(0)
        elif command == ("git", "rev-parse", "refs/remotes/origin/main"):
            stdout = self.origin_heads.pop(0)
        elif command == ("git", "tag", "--list", "v*"):
            stdout = "".join(f"{tag}\n" for tag in self.local_tags)
        elif command[:5] == ("git", "ls-remote", "--tags", "--refs", "origin"):
            pattern = command[5]
            if pattern == "refs/tags/v*":
                stdout = "".join(
                    f"{sha}\trefs/tags/{tag}\n"
                    for tag, sha in sorted(self.remote_tags.items())
                )
            else:
                tag = pattern.removeprefix("refs/tags/")
                if tag in self.remote_tags:
                    stdout = f"{self.remote_tags[tag]}\t{pattern}\n"
        elif command[:4] == ("gh", "api", "--hostname", "github.com"):
            returncode = self.release_returncode
            stderr = self.release_stderr
        elif command == ("just", "check") and self.on_check:
            self.on_check()
        return subprocess.CompletedProcess(command, returncode, stdout, stderr)


class VersionTests(unittest.TestCase):
    def test_package_version_comes_from_cargo_manifest(self) -> None:
        manifest = mock.mock_open(read_data=b'[package]\nversion = "12.34.56"\n')
        with mock.patch.object(Path, "open", manifest):
            self.assertEqual(cut_release.package_version(), "v12.34.56")
        manifest.assert_called_once_with("rb")

    def test_parse_version_is_ascii_semver(self) -> None:
        self.assertEqual(cut_release.parse_version("v12.34.56"), (12, 34, 56))
        invalid = (
            "0.1.0",
            "v0.1",
            "v0.1.0.0",
            "v00.1.0",
            "v0.01.0",
            "v0.1.00",
            "v٠.١.٠",
            "v０.１.０",
        )
        for version in invalid:
            with self.subTest(version=version), self.assertRaises(ValueError):
                cut_release.parse_version(version)

    def test_next_version_uses_manifest_and_rejects_released_version(self) -> None:
        with mock.patch.object(cut_release, "package_version", return_value=VERSION):
            self.assertEqual(cut_release.next_version([]), VERSION)
            with self.assertRaisesRegex(ValueError, "package version already released"):
                cut_release.next_version([VERSION])

    def test_version_queries_only_cross_documented_read_only_boundaries(self) -> None:
        expected_calls = [
            ("git", "remote", "get-url", "--all", "origin"),
            ("git", "remote", "get-url", "--push", "--all", "origin"),
            ("git", "tag", "--list", "v*"),
            ("git", "ls-remote", "--tags", "--refs", "origin", "refs/tags/v*"),
        ]
        for flag in ("--print-current-version", "--print-next-version"):
            with self.subTest(flag=flag):
                fake = FakeRun()
                with mock.patch.object(cut_release, "run", side_effect=fake), contextlib.redirect_stdout(
                    io.StringIO()
                ):
                    self.assertEqual(cut_release.main([flag]), 0)
                self.assertEqual(fake.calls, expected_calls)


class ReleaseTests(unittest.TestCase):
    def run_release(
        self,
        fake: FakeRun,
        *,
        dry_run: bool = False,
        notes_text: str = "Reviewed release notes\n",
        version: str = VERSION,
    ) -> tuple[list[tuple[str, ...]], list[str | None], str]:
        with tempfile.TemporaryDirectory() as directory:
            notes = Path(directory, "notes.md")
            notes.write_text(notes_text, encoding="utf-8")
            output = io.StringIO()
            with mock.patch.object(cut_release, "run", side_effect=fake), mock.patch(
                "cut_release.shutil.which", return_value="/bin/tool"
            ), contextlib.redirect_stdout(output):
                cut_release.release(version, notes, dry_run)
            return fake.calls, fake.inputs, output.getvalue()

    def test_just_boundary_passes_metacharacter_notes_path_as_one_argument(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory, "command-injection-marker")
            notes = Path(f"{directory}/notes||touch {marker}")
            notes.parent.mkdir(parents=True)
            notes.write_text("", encoding="utf-8")
            result = subprocess.run(
                (
                    "just",
                    "cut-release",
                    "--version",
                    VERSION,
                    "--notes-file",
                    str(notes),
                ),
                cwd=cut_release.ROOT,
                text=True,
                capture_output=True,
            )

            self.assertEqual(result.returncode, 2)
            self.assertIn("notes file must exist and be non-empty", result.stderr)
            self.assertFalse(marker.exists())

    def test_release_rejects_invalid_version_absent_and_blank_notes(self) -> None:
        fake = FakeRun()
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(
            cut_release, "run", side_effect=fake
        ):
            directory_path = Path(directory)
            with self.assertRaisesRegex(ValueError, "vMAJOR.MINOR.PATCH"):
                cut_release.release("0.1.0", directory_path / "missing", False)
            with self.assertRaisesRegex(ValueError, "notes file"):
                cut_release.release(VERSION, directory_path / "missing", False)
            blank = directory_path / "blank.md"
            blank.write_text(" \n\t", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "notes file"):
                cut_release.release(VERSION, blank, False)
        self.assertEqual(fake.calls, [])

    def test_release_rejects_dirty_wrong_or_diverged_main(self) -> None:
        cases = {
            "dirty": lambda fake: setattr(fake, "statuses", [" M README.md\n"]),
            "wrong branch": lambda fake: setattr(fake, "branch", "topic\n"),
            "diverged": lambda fake: setattr(fake, "origin_heads", ["b" * 40 + "\n"]),
        }
        for name, configure in cases.items():
            with self.subTest(name=name):
                fake = FakeRun()
                configure(fake)
                with self.assertRaises(RuntimeError):
                    self.run_release(fake)
                self.assertFalse(any(call[:2] == ("git", "push") for call in fake.calls))
                self.assertFalse(any(call[:3] == ("gh", "release", "create") for call in fake.calls))

    def test_release_version_must_match_cargo_manifest(self) -> None:
        fake = FakeRun()
        with self.assertRaisesRegex(ValueError, "version must match Cargo.toml"):
            self.run_release(fake, version="v0.1.0")
        self.assertFalse(any(call[:2] == ("git", "push") for call in fake.calls))

    def test_release_accepts_only_canonical_github_origin(self) -> None:
        accepted = (
            CANONICAL_ORIGIN,
            "https://github.com/iancleary/ccsds-ethernet-client.git",
            "ssh://git@github.com/iancleary/ccsds-ethernet-client",
        )
        rejected = (
            "git@evil.example:iancleary/ccsds-ethernet-client.git",
            "https://github.com/other/ccsds-ethernet-client.git",
            "https://github.com.evil.example/iancleary/ccsds-ethernet-client.git",
            "https://user@github.com/iancleary/ccsds-ethernet-client.git",
        )
        for origin in accepted:
            with self.subTest(origin=origin):
                fake = FakeRun()
                fake.origin = origin + "\n"
                with mock.patch.object(cut_release, "run", side_effect=fake):
                    cut_release.verify_origin()
        for origin in rejected:
            with self.subTest(origin=origin), self.assertRaises(RuntimeError):
                fake = FakeRun()
                fake.origin = origin + "\n"
                with mock.patch.object(cut_release, "run", side_effect=fake):
                    cut_release.verify_origin()

        malicious_push = FakeRun()
        malicious_push.push_origin = rejected[0] + "\n"
        with self.assertRaises(RuntimeError), mock.patch.object(
            cut_release, "run", side_effect=malicious_push
        ):
            cut_release.verify_origin()

    def test_existing_release_and_non_404_api_errors_stop_before_writes(self) -> None:
        cases = ((0, "", "already exists"), (1, "gh: timeout\n", "could not verify"))
        for returncode, stderr, message in cases:
            with self.subTest(message=message):
                fake = FakeRun()
                fake.release_returncode = returncode
                fake.release_stderr = stderr
                with self.assertRaisesRegex(RuntimeError, message):
                    self.run_release(fake)
                self.assertFalse(any(call[:2] == ("git", "push") for call in fake.calls))

    def test_post_check_dirtiness_or_divergence_stops_before_writes(self) -> None:
        dirty = FakeRun()
        dirty.statuses = ["", " M generated\n"]
        diverged = FakeRun()
        diverged.origin_heads = [SHA + "\n", "b" * 40 + "\n"]
        for name, fake in (("dirty", dirty), ("diverged", diverged)):
            with self.subTest(name=name), self.assertRaises(RuntimeError):
                self.run_release(fake)
            self.assertEqual(
                sum(call == ("git", "fetch", "--quiet", "origin", "main", "--tags") for call in fake.calls),
                1 if name == "dirty" else 2,
            )
            self.assertFalse(any(call[:2] == ("git", "push") for call in fake.calls))

    def test_post_check_revalidations_reject_concurrent_changes_before_writes(self) -> None:
        moved_sha = "b" * 40
        cases = (
            (
                "origin changed before second fetch",
                lambda fake: setattr(fake, "origin", "git@evil.example:owner/repository.git\n"),
                "origin fetch and push URLs must be the canonical",
                1,
            ),
            (
                "HEAD and origin/main moved together",
                lambda fake: (
                    setattr(fake, "heads", [moved_sha + "\n"]),
                    setattr(fake, "origin_heads", [moved_sha + "\n"]),
                ),
                "HEAD changed while repository checks ran",
                2,
            ),
            (
                "new tag conflicts with the validated SHA",
                lambda fake: fake.remote_tags.__setitem__(VERSION, moved_sha),
                "remote tag .* expected",
                2,
            ),
        )
        fetch = ("git", "fetch", "--quiet", "origin", "main", "--tags")
        for name, after_check, message, fetch_count in cases:
            with self.subTest(name=name):
                fake = FakeRun()
                fake.on_check = lambda: after_check(fake)
                with self.assertRaisesRegex((RuntimeError, ValueError), message):
                    self.run_release(fake)
                self.assertEqual(sum(call == fetch for call in fake.calls), fetch_count)
                self.assertFalse(any(call[:2] == ("git", "push") for call in fake.calls))
                self.assertFalse(any(call[:3] == ("gh", "release", "create") for call in fake.calls))

    def test_new_tag_is_pushed_at_validated_sha_then_release_is_created_last(self) -> None:
        fake = FakeRun()
        calls, inputs, _ = self.run_release(fake)
        push = ("git", "push", "origin", f"{SHA}:refs/tags/{VERSION}")
        create = (
            "gh",
            "release",
            "create",
            VERSION,
            "--repo",
            "github.com/iancleary/ccsds-ethernet-client",
            "--title",
            VERSION,
            "--notes-file",
            "-",
            "--verify-tag",
        )
        self.assertEqual(calls[-2:], [push, create])
        self.assertEqual(inputs[-1], "Reviewed release notes\n")
        self.assertIn(("gh", "auth", "status", "--hostname", "github.com"), calls)
        api = next(call for call in calls if call[:2] == ("gh", "api"))
        self.assertEqual(api[2:4], ("--hostname", "github.com"))

    def test_exact_tag_only_state_resumes_without_push_and_mismatch_is_rejected(self) -> None:
        exact = FakeRun()
        exact.remote_tags[VERSION] = SHA
        calls, _, _ = self.run_release(exact)
        self.assertFalse(any(call[:2] == ("git", "push") for call in calls))
        self.assertEqual(calls[-1][:3], ("gh", "release", "create"))

        mismatch = FakeRun()
        mismatch.remote_tags[VERSION] = "b" * 40
        with self.assertRaisesRegex(RuntimeError, "expected"):
            self.run_release(mismatch)
        self.assertFalse(any(call[:2] == ("git", "push") for call in mismatch.calls))
        self.assertFalse(any(call[:3] == ("gh", "release", "create") for call in mismatch.calls))

    def test_notes_are_snapshotted_once_and_sent_through_stdin(self) -> None:
        fake = FakeRun()
        with tempfile.TemporaryDirectory() as directory:
            notes = Path(directory, "notes.md")
            notes.write_text("reviewed snapshot\n", encoding="utf-8")
            fake.on_check = lambda: notes.write_text("changed after review\n", encoding="utf-8")
            with mock.patch.object(cut_release, "run", side_effect=fake), mock.patch(
                "cut_release.shutil.which", return_value="/bin/tool"
            ):
                cut_release.release(VERSION, notes, False)
        create_index = next(i for i, call in enumerate(fake.calls) if call[:3] == ("gh", "release", "create"))
        self.assertEqual(fake.inputs[create_index], "reviewed snapshot\n")
        self.assertEqual(fake.calls[create_index][-2:], ("-", "--verify-tag"))

    def test_dry_run_prints_both_writes_without_executing_either(self) -> None:
        fake = FakeRun()
        calls, _, output = self.run_release(fake, dry_run=True)
        self.assertFalse(any(call[:2] == ("git", "push") for call in calls))
        self.assertFalse(any(call[:3] == ("gh", "release", "create") for call in calls))
        self.assertIn(f"DRY RUN: git push origin {SHA}:refs/tags/{VERSION}", output)
        self.assertIn("DRY RUN: gh release create", output)
        self.assertIn("--notes-file - --verify-tag", output)

    def test_complete_safety_order_includes_post_check_revalidation(self) -> None:
        fake = FakeRun()
        calls, _, _ = self.run_release(fake)

        def index(command: tuple[str, ...], start: int = 0) -> int:
            return calls.index(command, start)

        fetch = ("git", "fetch", "--quiet", "origin", "main", "--tags")
        first_fetch = index(fetch)
        check = index(("just", "check"))
        post_status = index(("git", "status", "--porcelain"), check)
        second_fetch = index(fetch, first_fetch + 1)
        tag_query = index(("git", "ls-remote", "--tags", "--refs", "origin", f"refs/tags/{VERSION}"))
        api_query = next(i for i, call in enumerate(calls) if call[:2] == ("gh", "api"))
        push = next(i for i, call in enumerate(calls) if call[:2] == ("git", "push"))
        create = next(i for i, call in enumerate(calls) if call[:3] == ("gh", "release", "create"))
        self.assertLess(first_fetch, check)
        self.assertLess(check, post_status)
        self.assertLess(post_status, second_fetch)
        self.assertLess(second_fetch, tag_query)
        self.assertLess(tag_query, api_query)
        self.assertEqual((push, create), (len(calls) - 2, len(calls) - 1))


if __name__ == "__main__":
    unittest.main()
