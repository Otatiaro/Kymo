#!/usr/bin/env python3
"""Record controller.cpp on a NUCLEO-H563ZI through its ST-Link, and check
why its alarm comes.

    controller.py --image I --host KYMO --probe-rs P --probe-id SERIAL [--seconds 5] [--out CAPTURE]

Flashes the image with probe-rs, resets it, and records it with `kymo record`
through probe-rs for the given time, kept in CAPTURE when given. Then checks,
from the decoded events, that:

- no slot is lost, and the timer ticks once a millisecond;
- the alarm comes now and then: once at least, on at most a twentieth of the
  ticks;
- each alarm comes while a cycle is still open, and the link's handler began
  in the two periods before it: a burst delayed the tick that woke the cycle,
  or the cycle itself, past the next tick. The cause the image is built
  around, the one an agent is to find from the capture alone.

    controller.py --host KYMO --check CAPTURE

checks a capture already recorded, with no board.
"""

import argparse
import shutil
import subprocess
import sys
import tempfile

CHIP = "STM32H563ZITx"
NAMESPACE = "app::(anonymous namespace)::"
TIMER = f"isr_enter handler={NAMESPACE}timer_isr "
LINK = f"isr_enter handler={NAMESPACE}link_isr "
ALARM = f"{NAMESPACE}alarm "
BEGIN = f"kymo::begin_of<{NAMESPACE}cycle>"
END = f"kymo::end_of<{NAMESPACE}cycle>"
# A burst delays the alarm's cycle from the period before it: the tick that
# wakes the cycle waits behind it, or the cycle itself does.
DELAY_WINDOW = 2 * 8000


def run(*arguments: str) -> subprocess.CompletedProcess:
    return subprocess.run(list(arguments), capture_output=True, text=True, encoding="utf-8")


def check_alarms(decoded: str, seconds: int) -> list[str]:
    """The alarms against the cycles and the link's bursts, in event order."""
    errors = []
    ticks = alarms = cycles = 0
    bursts = []  # when each burst began, in cycles of the core
    cycle_open = False
    explained = 0
    for line in decoded.splitlines():
        if line.startswith("lost "):
            errors.append(f"slots lost: {line}")
            continue
        if not line.startswith("event "):
            continue
        cycles_at = int(line.split()[2])
        if TIMER in line:
            ticks += 1
        elif LINK in line:
            bursts.append(cycles_at)
        elif BEGIN in line:
            cycles += 1
            cycle_open = True
        elif END in line:
            cycle_open = False
        elif ALARM in line:
            alarms += 1
            if not cycle_open:
                errors.append(f"an alarm with no cycle open: {line}")
            elif not any(0 < cycles_at - burst < DELAY_WINDOW for burst in bursts[-2:]):
                errors.append(f"an alarm with no burst in the two periods before it: {line}")
            else:
                explained += 1
    print(f"{ticks} ticks, {cycles} cycles, {len(bursts)} bursts, {alarms} alarms, "
          f"{explained} of them after a burst")
    if abs(ticks - seconds * 1000) > seconds * 10:
        errors.append(f"{ticks} ticks in {seconds} s: not one a millisecond")
    if alarms == 0:
        errors.append("no alarm")
    if alarms > ticks / 20:
        errors.append(f"{alarms} alarms: more than one tick in twenty")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image")
    parser.add_argument("--host", required=True)
    parser.add_argument("--probe-rs")
    parser.add_argument("--probe-id")
    parser.add_argument("--seconds", type=int, default=5)
    parser.add_argument("--out")
    parser.add_argument("--check")
    args = parser.parse_args()

    if args.check:
        decoded = run(args.host, "decode", args.check)
        if decoded.returncode != 0:
            print(decoded.stderr)
            print("kymo decode failed")
            return 1
        return report(check_alarms(decoded.stdout, args.seconds))
    if not (args.image and args.probe_rs and args.probe_id):
        parser.error("--image, --probe-rs and --probe-id record a board; --check reads a capture")

    probe = f"0483:374e:{args.probe_id}"
    for command in ("download", "reset"):
        flashed = run(args.probe_rs, command, "--probe", probe, "--chip", CHIP,
                      *([args.image] if command == "download" else []))
        if flashed.returncode != 0:
            print(flashed.stderr)
            print(f"probe-rs {command} failed")
            return 1
    with tempfile.TemporaryDirectory() as directory:
        capture = f"{directory}/controller.kcap"
        record = subprocess.run(
            [args.host, "record", "--probe", "probe-rs", "--probe-id", probe, "--chip", CHIP,
             "--elf", args.image, "--duration", str(args.seconds), "--out", capture],
            stdin=subprocess.DEVNULL, capture_output=True, text=True, encoding="utf-8")
        print(record.stderr.strip())
        if record.returncode != 0:
            print("kymo record failed")
            return 1
        decoded = run(args.host, "decode", capture)
        if decoded.returncode != 0:
            print(decoded.stderr)
            print("kymo decode failed")
            return 1
        if args.out:
            shutil.copyfile(capture, args.out)
    return report(check_alarms(decoded.stdout, args.seconds))


def report(errors: list[str]) -> int:
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
