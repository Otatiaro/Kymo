#!/usr/bin/env python3
"""What tracing adds to each path of a bench, held against the record.

Runs OpSy's bench/profile.py on the bench image built without Kymo and on the
one built with the adapter: OpSy's bench with bench/opsy_hooks.hpp here, or
target/freertos/tests/bench/bench.cpp untraced and traced. It takes, for
each scenario, the instructions the traced image executes beyond the plain one.
That is the adapter's cost on the path: the events it emits, times what each
costs. The expected costs are in overhead-<target>.json; a path that costs
more than its record plus the tolerance fails, which an extra event on the
path or a dearer emit does. A drop beyond the tolerance is reported too, so
that the record follows the gain.

    overhead.py --profile <OpSy>/tests/qemu/bench/profile.py --machine mps2-an505
                --plain build-plain/opsy_qemu_bench.elf
                --traced build-traced/opsy/opsy_qemu_bench.elf
                --expected target/opsy/tests/bench/overhead-m33.json
                [--tolerance 10] [--json out.json] [--qemu ...] [--nm ...]
"""

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path


def medians(args, image: Path) -> dict:
    with tempfile.TemporaryDirectory() as directory:
        out = Path(directory) / "medians.json"
        subprocess.run(
            [sys.executable, str(args.profile), "--image", str(image), "--machine", args.machine,
             "--qemu", args.qemu, "--nm", args.nm, "--top", "0", "--json", str(out)],
            check=True, stdout=subprocess.DEVNULL)
        return json.loads(out.read_text())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", required=True, type=Path, help="OpSy's tests/qemu/bench/profile.py")
    parser.add_argument("--machine", required=True)
    parser.add_argument("--plain", required=True, type=Path, help="the bench image without Kymo")
    parser.add_argument("--traced", required=True, type=Path, help="the bench image with the adapter")
    parser.add_argument("--expected", type=Path, help="the recorded cost of each path")
    parser.add_argument("--tolerance", type=int, default=10, help="instructions; below one event's cost")
    parser.add_argument("--json", type=Path, help="write the measured costs here")
    parser.add_argument("--qemu", default="qemu-system-arm")
    parser.add_argument("--nm", default="arm-none-eabi-nm")
    args = parser.parse_args()

    plain = medians(args, args.plain)
    traced = medians(args, args.traced)
    if plain.keys() != traced.keys():
        print(f"the two images run different scenarios: {sorted(plain)} and {sorted(traced)}")
        return 1
    cost = {name: traced[name] - plain[name] for name in plain}

    expected = json.loads(args.expected.read_text()) if args.expected else {}
    failed = False
    for name in plain:
        line = f"{name:30} {plain[name]:5} -> {traced[name]:5}  +{cost[name]:4}  ({100 * cost[name] / plain[name]:5.1f} %)"
        if name in expected:
            difference = cost[name] - expected[name]
            if difference > args.tolerance:
                line += f"  GREW by {difference} over the recorded {expected[name]}"
                failed = True
            elif difference < -args.tolerance:
                line += f"  dropped by {-difference} under the recorded {expected[name]}: update the record"
        elif args.expected:
            line += "  not recorded"
            failed = True
        print(line)

    if args.json:
        args.json.write_text(json.dumps(cost, indent=2) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
