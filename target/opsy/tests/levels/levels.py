#!/usr/bin/env python3
"""Record the call levels image through QEMU's monitor, and check what the
host makes of its faults.

    levels.py --qemu Q --machine M --image I --host KYMO

Runs kymo_opsy_levels (levels.cpp) under QEMU, recorded by `kymo record`
from its first event to its exit, as trace.py records OpSy's suite. Then
checks that the image passed, and that `kymo decode --meaning` gives each
fault its place and its probable callers, nearest first:

- in a task: in that task, level_two, level_one, then in_a_task;
- in an interrupt handler: in its exception, installed at run time and not
  traced, level_two, level_one, then on_interrupt;
- on a stack not the task's: the stack not searched, level_two from lr when
  lr still holds level_three's return address (not at -Og, where level_three
  calls out first), else none;
- with a floating-point frame (both machines have an FPU): as in a task, under
  with_a_floating_point_frame, the decoy not among them.
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "tests", "record"))
from record import TIMEOUT_SECONDS, record_to_exit  # noqa: E402

# Each case's place, then the functions of its probable callers in order:
# None for those lr alone gives.
CASES = [
    ("in a task", "task=", ["level_two", "level_one", "in_a_task"]),
    ("in an interrupt", "exception=", ["level_two", "level_one", "on_interrupt"]),
    ("on a stack not its own", "task=", None),
    ("with a floating-point frame", "task=", ["level_two", "level_one", "with_a_floating_point_frame"]),
]


def callers(line: str) -> list[str]:
    """The functions of a fault line's probable callers, in order; none when
    it has none."""
    found = re.search(r"probable-callers=\[(.*?)\]", line)
    if not found:
        return []
    # "(anonymous namespace)::level_two+0xc at levels.cpp:146" gives level_two.
    return [re.sub(r"\+0x[0-9a-f]+$", "", caller.split(" at ")[0]).split("::")[-1]
            for caller in found.group(1).split(", ")]


def from_lr(line: str) -> list[str]:
    """level_two when the fault line's lr is in level_two, else none."""
    return ["level_two"] if re.search(r" lr=0x[0-9a-f]+ \(\(anonymous namespace\)::level_two\+", line) else []


def check(decoded: str) -> list[str]:
    faults = [line for line in decoded.splitlines() if line.startswith("fault ")]
    errors = []
    if len(faults) != len(CASES):
        errors.append(f"{len(faults)} fault lines, {len(CASES)} expected")
    for (name, place, expected), line in zip(CASES, faults):
        if expected is None:
            expected = from_lr(line)
        print(f"{name}: {line}")
        if f" {place}" not in line:
            errors.append(f"{name}: not {place}...")
        got = callers(line)
        if got != expected:
            errors.append(f"{name}: probable callers {got}, expected {expected}")
        if "decoy" in line:
            errors.append(f"{name}: the decoy among the callers")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    args = parser.parse_args()

    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "levels.kymo")
        recording = record_to_exit(args.qemu, args.machine, args.image, args.host, capture)
        if recording.returncode != 0:
            print(f"kymo record failed ({recording.returncode}): {recording.stderr.strip()}")
            return 1
        if recording.exit_status != 0:
            print(f"the image did not pass: exit status {recording.exit_status}")
            return 1
        decoded = subprocess.run([args.host, "decode", capture, "--meaning"], capture_output=True, text=True,
                                 encoding="utf-8", timeout=TIMEOUT_SECONDS)
    if decoded.returncode != 0:
        print(f"kymo decode --meaning failed: {decoded.stderr.strip()}")
        return 1
    errors = check(decoded.stdout)
    for error in errors:
        print(f"*** {error}")
    print("RESULT: FAIL" if errors else "every fault named its probable callers\nRESULT: PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
