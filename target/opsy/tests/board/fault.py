#!/usr/bin/env python3
"""Record fault.cpp on a NUCLEO-H563ZI through its ST-Link, and check the fault.

    fault.py --image I --host KYMO --probe-rs P --probe-id SERIAL [--seconds 5]

Flashes the image with probe-rs, resets it, and records it with `kymo record`
through probe-rs for the given time, long enough for the worker's hundred
samples and its fault. Then checks that:

- `kymo decode --meaning` gives one fault, in the worker task, a precise
  BusFault at the sensor's address, its pc at a source line of read_sensor,
  where the faulting load is, and its probable callers measure then worker;
- the fault is the last event of the capture: the application froze the
  trace in its handler, and nothing followed;
- `kymo check` fails on a rule of no fault, and passes on a rule of one.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

CHIP = "STM32H563ZITx"
SOURCE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fault.cpp")


def body() -> range:
    """The lines of read_sensor's body in fault.cpp, braces included: the
    faulting load is one of them, by the line the compiler gave it."""
    with open(SOURCE, encoding="utf-8") as file:
        lines = file.read().splitlines()
    start = next(n for n, line in enumerate(lines) if "read_sensor()" in line and "noinline" in line) + 1
    end = next(n for n in range(start, len(lines)) if lines[n] == "}") + 1
    return range(start + 1, end + 1)


def run(*arguments: str) -> subprocess.CompletedProcess:
    return subprocess.run(list(arguments), capture_output=True, text=True, encoding="utf-8")


def last_event(decoded: str) -> int | None:
    """The index of the last event kymo decode prints."""
    events = [re.match(r"event (\d+)", line) for line in decoded.splitlines() if line.startswith("event ")]
    return int(events[-1].group(1)) if events else None


def check_fault(meaning: str, last: int | None) -> list[str]:
    faults = [line for line in meaning.splitlines() if line.startswith("fault ")]
    if len(faults) != 1:
        return [f"{len(faults)} fault lines, one expected"]
    line = faults[0]
    print(line)
    errors = []
    at = re.search(r"^fault at=(\d+) ", line)
    if not at or last is None or int(at.group(1)) != last:
        errors.append(f"the fault is not the last event ({last}): something followed the freeze")
    expected = [
        (r" task=app::\(anonymous namespace\)::g_worker ", "not in the worker task"),
        (r" bits=PRECISERR,BFARVALID ", "not a precise BusFault"),
        (r" bfar=0xf0000000 ", "not at the sensor's address"),
        (r" stack=psp ", "not on the process stack"),
    ]
    errors += [f"the fault: {why}" for pattern, why in expected if not re.search(pattern, line)]
    pc = re.search(r" pc=0x[0-9a-f]+ \(app::\(anonymous namespace\)::read_sensor\+0x[0-9a-f]+ at [^)]*fault\.cpp:(\d+)\)", line)
    if not pc or int(pc.group(1)) not in body():
        errors.append(f"the fault: pc not in read_sensor, fault.cpp:{body().start}-{body().stop - 1}")
    if "frame=" in line:
        errors.append("the fault: its frame flagged")
    found = re.search(r"probable-callers=\[(.*?)\]", line)
    callers = [re.sub(r"\+0x[0-9a-f]+$", "", caller.split(" at ")[0]).split("::")[-1]
               for caller in found.group(1).split(", ")] if found else []
    if callers[:2] != ["measure", "worker"]:
        errors.append(f"the fault: probable callers {callers}, expected measure then worker")
    return errors


def check_rules(host: str, capture: str, directory: str) -> list[str]:
    errors = []
    for most, status in ((0, 3), (1, 0)):
        rules = os.path.join(directory, f"faults-{most}.json")
        with open(rules, "w", encoding="utf-8") as file:
            json.dump({"rules": [{"capture": "faults", "max": most}]}, file)
        done = run(host, "check", capture, rules)
        if done.returncode != status:
            errors.append(f"kymo check, at most {most} fault: status {done.returncode}, {status} expected")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    parser.add_argument("--probe-rs", required=True)
    parser.add_argument("--probe-id", required=True)
    parser.add_argument("--seconds", type=int, default=5)
    args = parser.parse_args()

    probe = f"0483:374e:{args.probe_id}"
    for command in ("download", "reset"):
        flashed = run(args.probe_rs, command, "--probe", probe, "--chip", CHIP,
                      *([args.image] if command == "download" else []))
        if flashed.returncode != 0:
            print(flashed.stderr)
            print(f"probe-rs {command} failed")
            return 1
    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "fault.kcap")
        record = subprocess.run(
            [args.host, "record", "--probe", "probe-rs", "--probe-id", probe, "--chip", CHIP,
             "--elf", args.image, "--duration", str(args.seconds), "--out", capture],
            stdin=subprocess.DEVNULL, capture_output=True, text=True, encoding="utf-8")
        print(record.stderr.strip())
        if record.returncode != 0:
            print("kymo record failed")
            return 1
        decoded = run(args.host, "decode", capture)
        meaning = run(args.host, "decode", capture, "--meaning")
        stats = run(args.host, "stats", capture, "--json")
        if any(done.returncode != 0 for done in (decoded, meaning, stats)):
            print(decoded.stderr + meaning.stderr + stats.stderr)
            print("kymo decode or stats failed")
            return 1
        report = json.loads(stats.stdout)
        errors = check_fault(meaning.stdout, last_event(decoded.stdout))
        if report["faults"] != 1:
            errors.append(f"kymo stats counts {report['faults']} faults, one expected")
        errors += check_rules(args.host, capture, directory)
    for error in errors:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
