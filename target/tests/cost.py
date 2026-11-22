#!/usr/bin/env python3
"""What an emit costs, counted under QEMU, against a threshold.

    cost.py --qemu Q --machine M --nm NM --size SIZE
            --image I --baseline B [--max-event 50] [--max-slot 15]

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
The exit status is 1 when a one-slot event costs more than --max-event, or
when a slot more costs more than --max-slot over the event one slot shorter.
It also reports the flash of each function and the sections' growth.
"""

import argparse
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
    parser.add_argument("--max-event", type=int, default=50, help="most instructions for a one-slot event")
    parser.add_argument("--max-slot", type=int, default=15, help="most instructions for each slot more")
    args = parser.parse_args()

    kymo, kymo_sizes = counts(args.qemu, args.machine, args.nm, args.image)
    base, base_sizes = counts(args.qemu, args.machine, args.nm, args.baseline)

    over = []
    previous = None
    print(f"{'case':<16}{'instructions':>14}{'flash (bytes)':>15}")
    for name in CASES:
        if kymo[name] == 0 or base[name] == 0:
            print(f"{name} never ran")
            return 1
        cost = kymo[name] - base[name]
        flash = kymo_sizes[name] - base_sizes[name]
        print(f"{name.removeprefix('cost_'):<16}{cost:>14}{flash:>15}")
        if name == "cost_slots_1" and cost > args.max_event:
            over.append(f"{name}: {cost} > {args.max_event}")
        elif name.startswith("cost_slots_") and previous is not None and cost - previous > args.max_slot:
            over.append(f"{name}: a slot more costs {cost - previous} > {args.max_slot}")
        previous = cost

    growth = [k - b for k, b in zip(sections(args.size, args.image), sections(args.size, args.baseline))]
    print(f"sections: text +{growth[0]}, data +{growth[1]}, bss +{growth[2]} bytes")

    if over:
        print("over the threshold: " + "; ".join(over))
        return 1
    print(f"within the threshold: {args.max_event} for one slot, {args.max_slot} for each slot more")
    return 0


if __name__ == "__main__":
    sys.exit(main())
