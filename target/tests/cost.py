#!/usr/bin/env python3
"""What an emit costs, counted under QEMU, held against the record.

    cost.py --qemu Q --machine M --nm NM --size SIZE
            --image I --baseline B --expected cost/cost-<cpu>-<level>.json
            [--tolerance 2] [--json out.json]

Runs both images of target/tests/cost (with Kymo, and the baseline whose
functions do nothing) and counts the instructions each cost_* function
executes. QEMU does not model cycles; with one instruction per translation
block and the execution trace on (-accel tcg,one-insn-per-tb=on
-d exec,nochain), each trace line is one executed instruction, which is exact
and repeatable. Cycles are measured on hardware, not here.

Each function is called once, from test_main; its count runs from its entry
until execution is back in test_main, so what it calls counts too. Its cost
is its count with Kymo less its count in the baseline: the call and building
the payload are left out.
The expected costs, the disabled path included, are in the record of the CPU
and level. The exit status is 1 when a case costs more than its record plus
the tolerance, or when a case is missing from the record or from the image.
A drop beyond the tolerance is reported, so that the record follows the gain.
It also reports the flash of each function and the sections' growth.
"""

import argparse
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

EXEC = re.compile(r"^Trace \d+: \S+ \[[0-9a-f]+/([0-9a-f]+)/")
FUNCTION = re.compile(r"^([0-9a-f]+) ([0-9a-f]+) [Tt] (cost_\w+|_Z9test_mainv)$")
CALLER = "_Z9test_mainv"
CASES = ["cost_slots_1", "cost_slots_2", "cost_slots_3", "cost_slots_4", "cost_disabled"]


def functions(nm, image):
    """{name: (address, size)} of the image's cost_* functions."""
    text = subprocess.run([nm, "-S", "--defined-only", str(image)],
                          capture_output=True, text=True, check=True).stdout
    found = {}
    for line in text.splitlines():
        match = FUNCTION.match(line)
        if match:
            # Thumb: the symbol may carry the low bit.
            found[match.group(3)] = (int(match.group(1), 16) & ~1, int(match.group(2), 16))
    missing = [name for name in CASES + [CALLER] if name not in found]
    if missing:
        sys.exit(f"{image}: missing {', '.join(missing)}")
    return found


def executed(qemu, machine, image):
    """The addresses of the instructions the image executes, in order."""
    with tempfile.TemporaryDirectory() as directory:
        trace = Path(directory) / "trace.log"
        command = [
            qemu, "-machine", machine, "-accel", "tcg,one-insn-per-tb=on",
            "-kernel", str(image), "-nographic", "-semihosting-config", "enable=on,target=native",
            "-monitor", "none", "-serial", "none", "-d", "exec,nochain", "-D", str(trace),
        ]
        completed = subprocess.run(command, capture_output=True, text=True, timeout=300)
        output = completed.stdout + completed.stderr
        if "RESULT: PASS" not in output or completed.returncode != 0:
            sys.exit(f"{image} did not pass under QEMU:\n{output}")
        addresses = []
        with open(trace, encoding="utf-8", errors="replace") as lines:
            for line in lines:
                match = EXEC.match(line)
                if match:
                    addresses.append(int(match.group(1), 16))
        return addresses


def counts(qemu, machine, nm, image):
    """{name: instructions executed from its entry until back in test_main},
    what it calls included, and the functions' sizes."""
    found = functions(nm, image)
    entries = {found[name][0]: name for name in CASES}
    caller_start, caller_size = found[CALLER]
    result = {name: 0 for name in CASES}
    current = None
    for pc in executed(qemu, machine, image):
        if current is None:
            current = entries.get(pc)
        elif caller_start <= pc < caller_start + caller_size:
            current = entries.get(pc)
        if current is not None:
            result[current] += 1
    return result, {name: found[name][1] for name in CASES}


def sections(size_tool, image):
    """text, data and bss of the image, from size's Berkeley format."""
    lines = subprocess.run([size_tool, str(image)], capture_output=True, text=True, check=True).stdout.splitlines()
    text, data, bss = (int(value) for value in lines[1].split()[:3])
    return text, data, bss


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--nm", required=True)
    parser.add_argument("--size", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--expected", required=True, type=Path, help="the recorded cost of each case")
    parser.add_argument("--tolerance", type=int, default=2, help="instructions")
    parser.add_argument("--json", type=Path, help="write the measured costs here")
    args = parser.parse_args()

    kymo, kymo_sizes = counts(args.qemu, args.machine, args.nm, args.image)
    base, base_sizes = counts(args.qemu, args.machine, args.nm, args.baseline)

    expected = json.loads(args.expected.read_text())
    unknown = sorted(set(expected) - set(CASES))
    if unknown:
        print(f"{args.expected}: records cases the image has not: {', '.join(unknown)}")
        return 1

    failed = False
    measured = {}
    print(f"{'case':<16}{'instructions':>14}{'flash (bytes)':>15}")
    for name in CASES:
        if kymo[name] == 0 or base[name] == 0:
            print(f"{name} never ran")
            return 1
        cost = kymo[name] - base[name]
        measured[name] = cost
        flash = kymo_sizes[name] - base_sizes[name]
        line = f"{name.removeprefix('cost_'):<16}{cost:>14}{flash:>15}"
        if name not in expected:
            line += "  not recorded"
            failed = True
        else:
            difference = cost - expected[name]
            if difference > args.tolerance:
                line += f"  GREW by {difference} over the recorded {expected[name]}"
                failed = True
            elif difference < -args.tolerance:
                line += f"  dropped by {-difference} under the recorded {expected[name]}: update the record"
        print(line)

    growth = [k - b for k, b in zip(sections(args.size, args.image), sections(args.size, args.baseline))]
    print(f"sections: text +{growth[0]}, data +{growth[1]}, bss +{growth[2]} bytes")

    if args.json:
        args.json.write_text(json.dumps(measured, indent=2) + "\n")
    return 1 if failed else 0

if __name__ == "__main__":
    sys.exit(main())
