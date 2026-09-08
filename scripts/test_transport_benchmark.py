#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Executable JSON and accounting contract; optional disposable Linux proof."""
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BenchmarkTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        built = subprocess.run(
            ["cargo", "build", "--locked", "--release", "--example", "transport_benchmark", "--message-format=json"],
            cwd=ROOT, check=True, capture_output=True, text=True,
        )
        binaries = [entry["executable"] for line in built.stdout.splitlines()
                    if (entry := json.loads(line)).get("executable")
                    and entry.get("target", {}).get("name") == "transport_benchmark"]
        if len(binaries) != 1:
            raise AssertionError("expected one benchmark executable")
        cls.binary = binaries[0]

    def benchmark(self, *args, veth=False):
        command = ([sys.executable, str(ROOT / "scripts/test_linux_live.py"), "--release", "--benchmark"]
                   if veth else [self.binary])
        completed = subprocess.run(
            [*command, "--rates", "1000", "--samples", "12", "--capacity", "64", *args],
            cwd=ROOT, capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        rows = [json.loads(line) for line in completed.stdout.splitlines()]
        self.assertEqual(len(rows), 2)
        self.assertEqual([row["schema_version"] for row in rows], [1, 1])
        self.assertEqual(rows[-1]["type"], "sweep_summary")
        return rows

    def assert_clean(self, trial):
        self.assertFalse(trial["debug_build"])
        self.assertEqual(trial["sent"], 12)
        self.assertEqual(trial["missing_acks"], 0)
        self.assertEqual(trial["missing_telemetry"], 0)
        self.assertEqual(trial["command_rtt"]["samples"], 12)
        self.assertEqual(trial["telemetry_age"]["samples"],
                         12 * trial["telemetry_per_command"] + trial["background_telemetry_planned"])
        self.assertGreater(trial["allocation_calls"], 0)
        self.assertGreater(trial["allocation_requested_bytes"], 0)
        self.assertGreater(trial["reopen_to_echo_ns"], 0)
        self.assertLessEqual(trial["host_queue"]["high_water"], 64)
        self.assertGreater(trial["accounting_ns"], 0)

    def test_simulated_exchange_and_json_metrics(self):
        trial, summary = self.benchmark()
        self.assert_clean(trial)
        self.assertGreater(trial["achieved_offer_rate_per_s"], 0)
        self.assertEqual(summary["highest_tested_rate_without_observed_loss"], 1000)
        self.assertIsNone(summary["first_tested_rate_with_observed_loss_or_error"])

    def test_scripted_ack_loss_is_distinct_from_telemetry_loss(self):
        trial, summary = self.benchmark("--burst", "12", "--drop-every", "4")
        self.assertEqual(trial["intentional_ack_drops"], 3)
        self.assertEqual(trial["missing_acks"], 3)
        self.assertEqual(trial["command_rtt"]["samples"], 9)
        self.assertEqual(trial["telemetry_age"]["samples"], 12)
        self.assertIsNone(trial["achieved_offer_rate_per_s"])
        self.assertTrue(summary["fault_injection_enabled"])

    def test_empty_distributions_are_null_not_zero_latency(self):
        trial, _ = self.benchmark("--drop-every", "1", "--telemetry-per-command", "0")
        self.assertIsNone(trial["command_rtt"])
        self.assertIsNone(trial["telemetry_age"])
        self.assertEqual(trial["missing_acks"], 12)

    def test_background_telemetry_runs_independently_of_commands(self):
        trial, _ = self.benchmark("--telemetry-per-command", "0", "--telemetry-rate", "2000")
        self.assertEqual(trial["background_telemetry_planned"], 24)
        self.assertEqual(trial["peer_telemetry_attempted"], 24)
        self.assertEqual(trial["telemetry_age"]["samples"], 24)
        self.assertEqual(trial["missing_telemetry"], 0)

    def test_invalid_configuration_emits_no_partial_report(self):
        result = subprocess.run([self.binary, "--samples", "0"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")

    @unittest.skipUnless(os.environ.get("CCSDS_BENCHMARK_VETH") == "1", "requires Linux namespace permissions")
    def test_native_veth_same_measurement_contract(self):
        trial, _ = self.benchmark("--payload-bytes", "1472", "--telemetry-rate", "2000", veth=True)
        self.assertEqual(trial["backend"], "veth")
        self.assert_clean(trial)
        self.assertGreater(trial["cpu_process_ns"], 0)


if __name__ == "__main__":
    unittest.main()
