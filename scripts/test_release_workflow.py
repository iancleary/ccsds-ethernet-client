#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Check the registry publication order and credential boundaries."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")


class ReleaseWorkflowContractTests(unittest.TestCase):
    def test_all_artifacts_are_built_before_the_first_publish(self) -> None:
        self.assertIn(
            "needs: [verify, build-python-wheels, build-python-sdist]",
            WORKFLOW,
        )

    def test_pypi_publish_waits_for_crates_io(self) -> None:
        publish_pypi = WORKFLOW.index("  publish-pypi:")
        self.assertIn("needs: publish-crate", WORKFLOW[publish_pypi:])

    def test_oidc_permission_is_scoped_to_pypi_job(self) -> None:
        self.assertEqual(WORKFLOW.count("id-token: write"), 1)
        publish_pypi = WORKFLOW.index("  publish-pypi:")
        self.assertIn("id-token: write", WORKFLOW[publish_pypi:])

    def test_crates_token_is_scoped_to_publish_step(self) -> None:
        self.assertEqual(WORKFLOW.count("\n          CARGO_REGISTRY_TOKEN:"), 1)
        publish_crate = WORKFLOW.index("  publish-crate:")
        publish_pypi = WORKFLOW.index("  publish-pypi:")
        self.assertIn(
            "CARGO_REGISTRY_TOKEN",
            WORKFLOW[publish_crate:publish_pypi],
        )


if __name__ == "__main__":
    unittest.main()
