#!/usr/bin/env python3
"""Record the FreeRTOS test application on a NUCLEO-H563ZI through its ST-Link,
and check the trace.

    board.py --image I --host KYMO --probe-rs P --probe-id SERIAL [--nm NM] [--seconds 6]

The image is application.cpp built with KYMO_BOARD. Flashes it with probe-rs
and resets the board; the application waits for the probe 3 s on SysTick
before its first event, which `kymo record` attaches within, so that the
recording starts with it. Records for the given time through probe-rs (not
the stlink backend: STM32CubeProgrammer, attaching to an STM32H5 even in
hot-plug mode, halts the core and disables every interrupt), then reads
application::verdict through the probe: 1 once the application passed.

Then checks the capture as trace/trace.py checks QEMU's: the events against
the adapter's and the scenarios', no event lost, the core clock the board's
32 MHz; kymo decode --meaning's lines against the scenarios' wakes; and
board/rules.json with kymo check, which bounds the latencies the scenarios
make on the board.
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "trace"))
from trace import check, check_meaning  # noqa: E402

CHIP = "STM32H563ZITx"
CORE_CLOCK = "32000000"  # configCPU_CLOCK_HZ with KYMO_BOARD
VERDICT = "application::verdict"
RULES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rules.json")


def run(command: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, text=True, encoding="utf-8")


def verdict_address(nm: str, image: str) -> int | None:
    """application::verdict's address, from the image's symbols."""
    symbols = run([nm, "-C", image])
    for line in symbols.stdout.splitlines():
        words = line.split(maxsplit=2)
        if len(words) == 3 and words[2] == VERDICT:
            return int(words[0], 16)
    return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    parser.add_argument("--probe-rs", required=True)
    parser.add_argument("--probe-id", required=True)
    parser.add_argument("--nm", default="arm-none-eabi-nm")
    parser.add_argument("--seconds", type=int, default=6)
    args = parser.parse_args()

    address = verdict_address(args.nm, args.image)
    if address is None:
        print(f"no {VERDICT} in {args.image}: not built with KYMO_BOARD")
        return 1
    probe = f"0483:374e:{args.probe_id}"
    target = ["--probe", probe, "--chip", CHIP]
    for command in (["download", *target, args.image], ["reset", *target]):
        done = run([args.probe_rs, *command])
        if done.returncode != 0:
            print(done.stderr)
            print(f"probe-rs {command[0]} failed")
            return 1

    errors = []
    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "board.kcap")
        record = run([args.host, "record", "--probe", "probe-rs", "--probe-id", probe, "--chip", CHIP,
                      "--elf", args.image, "--duration", str(args.seconds), "--out", capture])
        print(record.stderr.strip())
        if record.returncode != 0:
            print("kymo record failed")
            return 1
        read = run([args.probe_rs, "read", *target, "b32", hex(address), "1"])
        words = read.stdout.split()
        if read.returncode != 0 or not words:
            errors.append(f"probe-rs read of {VERDICT} failed: {read.stderr.strip()}")
        elif int(words[-1], 16) != 1:
            errors.append(f"{VERDICT} is {words[-1]}: the application "
                          + ("did not end" if int(words[-1], 16) == 0 else "failed"))

        decoded = run([args.host, "decode", capture])
        meaning = run([args.host, "decode", capture, "--meaning"])
        checked = run([args.host, "check", capture, RULES])
        if decoded.returncode != 0 or meaning.returncode != 0:
            print(decoded.stderr + meaning.stderr)
            print("kymo decode failed")
            return 1
        errors += check(decoded.stdout, CORE_CLOCK)
        errors += check_meaning(meaning.stdout)
        print(checked.stdout.strip())
        if checked.returncode != 0:
            errors.append(f"kymo check: exit {checked.returncode} {checked.stderr.strip()}")
        lost = re.search(r"(\d+) slots lost", record.stderr)
        if lost is None or int(lost.group(1)) != 0:
            errors.append(f"slots lost while recording: {lost.group(1) if lost else 'no summary'}")
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
