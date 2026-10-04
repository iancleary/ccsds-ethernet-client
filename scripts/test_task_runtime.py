#!/usr/bin/env -S uv run --managed-python --python 3.11 --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Offline task-boundary runtime and Linux privilege/argument regression checks."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


class TaskRuntimeTests(unittest.TestCase):
    def test_actual_tasks_ignore_unsupported_ambient_python(self):
        with tempfile.TemporaryDirectory() as directory:
            fake = Path(directory)
            marker = fake / 'ambient-invoked'
            for name in ('python', 'python3'):
                executable = fake / name
                executable.write_text('#!/bin/sh\nprintf invoked > "' + str(marker) + '"\nexit 93\n')
                executable.chmod(0o755)
            env = dict(os.environ, PATH=str(fake) + os.pathsep + os.environ['PATH'],
                       UV_PYTHON_DOWNLOADS='never', UV_OFFLINE='1')
            for task in ('linux-test', 'linux-benchmark', 'cut-release'):
                with self.subTest(task=task):
                    result = subprocess.run([shutil.which('just'), task, '--help'],
                                            cwd=ROOT, env=env, capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn('usage:', result.stdout)
            self.assertFalse(marker.exists(), 'Task invoked unsupported ambient Python')

    def test_literal_arguments_survive_actual_just_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            fake = Path(directory)
            record = fake / 'arguments.json'
            # Observe argv at uv's boundary without executing transport or publication.
            executable = fake / 'uv'
            executable.write_text('#!' + os.sys.executable + '\nimport json,sys\nfrom pathlib import Path\nPath(' + repr(str(record)) + ').write_text(json.dumps(sys.argv[1:]))\n')
            executable.chmod(0o755)
            env = dict(os.environ, PATH=str(fake) + os.pathsep + os.environ['PATH'])
            literal = 'space ; $(touch should-not-exist) "quoted" *'
            for task in ('linux-test', 'linux-benchmark', 'cut-release'):
                with self.subTest(task=task):
                    subprocess.run([shutil.which('just'), task, literal], cwd=ROOT,
                                   env=env, check=True, capture_output=True)
                    args = json.loads(record.read_text())
                    self.assertEqual(args[-1], literal)
                    self.assertIn('--managed-python', args)
                    self.assertEqual(args[args.index('--python') + 1], '3.11')

    def test_linux_permissions_and_wheel_path_preserved(self):
        spec = importlib.util.spec_from_file_location('linux_fixture', ROOT / 'scripts/test_linux_live.py')
        helper = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(helper)
        wheel = '/tmp/fixture wheel/bin/python'
        for uid in (0, 1000):
            with self.subTest(uid=uid):
                calls = []
                def run(command, **kwargs):
                    calls.append(command)
                    return subprocess.CompletedProcess(command, 0, stdout=json.dumps({
                        'reason': 'compiler-artifact', 'executable': '/tmp/fixture binary',
                        'target': {'name': 'linux_live'}}))
                with patch.object(helper.sys, 'argv', ['test_linux_live.py', '--python', wheel]), \
                        patch.object(helper.sys, 'platform', 'linux'), \
                        patch.object(helper.os, 'geteuid', return_value=uid), \
                        patch.object(helper.subprocess, 'run', side_effect=run):
                    helper.main()
                self.assertEqual(calls[0][0], 'cargo')  # Build remains unprivileged.
                prefix = [] if uid == 0 else ['sudo', '-n']
                self.assertTrue(all(list(command[:len(prefix)]) == prefix for command in calls[1:]))
                self.assertIn(wheel, calls[-2])
                self.assertEqual(list(calls[-1][-3:-1]), ['netns', 'delete'])


if __name__ == '__main__':
    unittest.main()
