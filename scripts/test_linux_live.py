#!/usr/bin/env python3
"""Build as the current user; run only the test executable in a disposable netns."""
import json
import argparse
import os
from pathlib import Path
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[1]

def run(*args):
    subprocess.run(args, check=True, cwd=ROOT)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", help="Installed-wheel Python executable for native smoke tests")
    parser.add_argument("--release", action="store_true", help="Measure an optimized test build")
    parser.add_argument("--benchmark", action="store_true", help="Run only the workload benchmark; remaining flags configure it")
    args, benchmark_args = parser.parse_known_args()
    if benchmark_args and not args.benchmark:
        parser.error("unknown test arguments: " + " ".join(benchmark_args))
    if args.benchmark and (args.python or "--backend" in benchmark_args):
        parser.error("benchmark mode owns its backend and does not run Python smoke tests")
    if sys.platform != "linux":
        raise SystemExit("live transport tests require Linux and iproute2")
    profile = ["--release"] if args.release else []
    target = "transport_benchmark" if args.benchmark else "linux_live"
    command = ["build", "--example", target] if args.benchmark else ["test", "--test", target, "--no-run"]
    build = subprocess.run(["cargo", *command, "--locked", *profile, "--message-format=json"], cwd=ROOT, check=True, capture_output=True, text=True)
    executables = [item["executable"] for line in build.stdout.splitlines() if (item := json.loads(line)).get("reason") == "compiler-artifact" and item.get("executable") and item["target"]["name"] == target]
    if len(executables) != 1:
        raise SystemExit("could not resolve unique Linux test executable")
    ns = "ccsds-test-" + uuid.uuid4().hex[:12]
    prefix = [] if os.geteuid() == 0 else ["sudo", "-n"]
    run(*prefix, "ip", "netns", "add", ns)
    try:
        run(*prefix, "ip", "-n", ns, "link", "add", "ccsds-host", "type", "veth", "peer", "name", "ccsds-peer")
        for name, mac in [("ccsds-host", "02:00:00:00:00:01"), ("ccsds-peer", "02:00:00:00:00:02")]:
            run(*prefix, "ip", "-n", ns, "link", "set", name, "address", mac)
            run(*prefix, "ip", "-n", ns, "link", "set", name, "up")
        run_args = ["--backend", "veth", *benchmark_args] if args.benchmark else ["--ignored", "--test-threads=1", "--nocapture"]
        run(*prefix, "ip", "netns", "exec", ns, "env", "CCSDS_LIVE_TEST=1", executables[0], *run_args)
        if args.python:
            # Preserve the venv executable path: resolving its symlink loses
            # the environment and can import a different installed package.
            run(*prefix, "ip", "netns", "exec", ns, "env", "CCSDS_LIVE_TEST=1", str(Path(args.python).absolute()), str(ROOT / "scripts/linux_python_smoke.py"))
    finally:
        run(*prefix, "ip", "netns", "delete", ns)

if __name__ == "__main__":
    main()
