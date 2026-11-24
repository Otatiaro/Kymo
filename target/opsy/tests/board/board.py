#!/usr/bin/env python3
"""Record board.cpp on a NUCLEO-H563ZI through its ST-Link, and check the trace.

    board.py --image I --host KYMO --probe-rs P --probe-id SERIAL [--seconds 30]

Flashes the image with probe-rs, records it with `kymo record` for the given
time through probe-rs, the cycle counter's frequency measured, and decodes the
capture. Not through the stlink backend: STM32CubeProgrammer, attaching to an
STM32H5 even in hot-plug mode, halts the core and disables every interrupt.
Then checks, besides trace.py's checks of the events and of `kymo stats`
against oracle.py, here with real durations and the middle half cutting
instances:

- that interrupts nest: every entry of the nested handler is inside the
  timer's, and every timer period has one;
- that every interval between two timer entries is one period: none missed,
  none taken twice. An entry is late by what masks the timer when it fires,
  OpSy's kernel and the sampler's lock, which the intervals' spread shows;
- that the intervals average 8000 cycles within 0.1: the core and the timer
  run on the same HSE, so the period holds a whole number of cycles, which
  checks the timer;
- that they average the nominal millisecond within 1e-4 in the seconds Kymo
  gives, which checks Kymo's measure of the clock against the HSE, which the
  ST-Link derives from its crystal. That measure holds while the probe keeps
  up with the ring: at the default 24 MHz of SWD it repeats within 1e-7; at
  4 MHz, where the ring overtakes the probe, it strays by up to 2.4e-4.
"""

import argparse
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "trace"))
from trace import parse, check, check_rules, check_stats  # noqa: E402

CHIP = "STM32H563ZITx"
PERIOD = 1e-3  # seconds
PERIOD_CYCLES = 8000  # of the core's 8 MHz
TIMER = "app::(anonymous namespace)::timer_isr"
NESTED = "app::(anonymous namespace)::nested_isr"


def check_board(output: str) -> list[str]:
    """Nesting and the timer's period, from the decoded output (with seconds)."""
    errors = []
    open_isrs = []
    entries = []  # (seconds, after a loss)
    nested = lost = 0
    timer_open_nested = None  # nested entries in the timer entry still open
    unnested = 0
    for line, name, fields in parse(output):
        if name is None:
            open_isrs, lost = [None], lost + 1
            timer_open_nested = None
            continue
        values = dict(fields)
        handler = values.get("handler")
        if name.endswith("::isr_enter"):
            if handler == TIMER:
                words = line.split()
                entries.append((int(words[2]), float(words[3]), lost))
                timer_open_nested = 0
            elif handler == NESTED:
                nested += 1
                if open_isrs and open_isrs[-1] == TIMER:
                    timer_open_nested += 1
                elif open_isrs != [None]:
                    errors.append(f"{line}: not nested in {TIMER}, open: {open_isrs}")
            open_isrs.append(handler)
        elif name.endswith("::isr_exit") and open_isrs and open_isrs != [None]:
            open_isrs.pop()
            if handler == TIMER and timer_open_nested is not None:
                if timer_open_nested != 1:
                    unnested += 1
                timer_open_nested = None
    if unnested:
        errors.append(f"{unnested} timer periods without exactly one nested interrupt")
    if nested == 0:
        errors.append("no nested interrupt recorded")
    if len(entries) < 2:
        return errors + [f"{len(entries)} timer interrupts recorded"]
    # Each entry's period: an interval is one period, or across a loss a whole
    # number of them, each rounded on its own, where the drift a wrong period
    # makes is far below half a period.
    strays = 0
    index = [0]
    intervals = []
    for (cycles_a, _, lost_a), (cycles_b, _, lost_b) in zip(entries, entries[1:]):
        periods = round((cycles_b - cycles_a) / PERIOD_CYCLES)
        if periods != 1 and (lost_a == lost_b or periods == 0):
            strays += 1
        elif lost_a == lost_b:
            intervals.append(cycles_b - cycles_a)
        index.append(index[-1] + max(periods, 1))
    if strays:
        errors.append(f"{strays} intervals between timer interrupts are not one period")
    # The period is the slope of the entries against their period, fitted over
    # the whole recording: an entry's lateness weighs on it as little as it can.
    # Not the span over a rounded number of periods, a rounding that would
    # absorb any error under half a period over the span.
    cycles = slope(index, [entry[0] for entry in entries])
    seconds = slope(index, [entry[1] for entry in entries])
    deviation = seconds / PERIOD - 1
    print(f"{len(entries)} timer interrupts over {index[-1]} periods, {nested} nested; intervals of "
          f"{min(intervals, default=0)} to {max(intervals, default=0)} cycles; period {cycles:.4f} cycles, "
          f"{seconds * 1e3:.7f} ms ({deviation:+.2e})")
    # The timer counts the core's clock: the period is a whole number of cycles.
    if abs(cycles - PERIOD_CYCLES) > 0.1:
        errors.append(f"the timer's period is {cycles:.4f} cycles, not {PERIOD_CYCLES}")
    if abs(deviation) > 1e-4:
        errors.append(f"the timer's period deviates by {deviation:+.2e} from {PERIOD} s")
    return errors


def slope(x: list[int], y: list[float]) -> float:
    """The least-squares slope of y against x."""
    mean_x, mean_y = sum(x) / len(x), sum(y) / len(y)
    return (sum((a - mean_x) * (b - mean_y) for a, b in zip(x, y))
            / sum((a - mean_x) ** 2 for a in x))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    parser.add_argument("--probe-rs", required=True)
    parser.add_argument("--probe-id", required=True)
    parser.add_argument("--seconds", type=int, default=30)
    args = parser.parse_args()

    probe = f"0483:374e:{args.probe_id}"
    for command in ("download", "reset"):
        flashed = subprocess.run([args.probe_rs, command, "--probe", probe, "--chip", CHIP]
                                 + ([args.image] if command == "download" else []),
                                 capture_output=True, text=True)
        if flashed.returncode != 0:
            print(flashed.stderr)
            print(f"probe-rs {command} failed")
            return 1
    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "board.kcap")
        record = subprocess.run(
            [args.host, "record", "--probe", "probe-rs", "--probe-id", probe, "--chip", CHIP,
             "--elf", args.image, "--duration", str(args.seconds), "--out", capture],
            stdin=subprocess.DEVNULL, capture_output=True, text=True)
        print(record.stderr.strip())
        if record.returncode != 0:
            print("kymo record failed")
            return 1
        decode = subprocess.run([args.host, "decode", args.image, capture], capture_output=True, text=True)
        if decode.returncode != 0:
            print(decode.stderr)
            print("kymo decode failed")
            return 1
        errors = (check(decode.stdout) + check_board(decode.stdout) + check_stats(args.host, args.image, capture)
                  + check_rules(args.host, args.image, capture))
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
