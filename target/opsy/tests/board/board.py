#!/usr/bin/env python3
"""Record board.cpp on a NUCLEO-H563ZI through its ST-Link, and check the trace.

    board.py --image I --host KYMO --probe-rs P --probe-id SERIAL [--seconds 30]

Flashes the image with probe-rs, records it with `kymo record` for the given
time through probe-rs, the cycle counter's frequency measured, and decodes the
capture. Not through the stlink backend: STM32CubeProgrammer, attaching to an
STM32H5 even in hot-plug mode, halts the core and disables every interrupt.
Then checks, besides trace.py's checks of the events and of `kymo stats`
against oracle.py, here with real durations and the middle half cutting
instances, the application's `sampling` durations among them, bounded by
`kymo check` rules:

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
  4 MHz, where the ring overtakes the probe, it strays by up to 2.4e-4;
- that no slot is lost: at 24 MHz the probe reads several times what the
  board writes, so a loss is the host falling behind;
- that the setpoints' half sine runs from 0 to its peak, and that `kymo check`
  bounds its excursion: a rule at the lowest and the highest position decoded
  passes, one a micrometre inside either fails;
- that kymo::dwt configured the data trace, read from the registers after the
  reset: the anchor on slot 0's timestamp word through the least capable
  comparator, g_position on another, the ITM and the TPIU sending SWO at
  8 MBd, the exceptions traced and a tick every 16384 cycles. The comparators keep their functions
  across the reset: what the run before left is freed, and a debugger's
  watchpoint, written before the reset, is kept.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "trace"))
from trace import SAMPLING, parse, check, check_rules, check_stats  # noqa: E402

CHIP = "STM32H563ZITx"
TICK_FIELDS = (1 << 22) | (1 << 9) | (0xF << 1)  # DWT_CTRL's CYCEVTENA, CYCTAP, POSTPRESET
PERIOD = 1e-3  # seconds
PERIOD_CYCLES = 8000  # of the core's 8 MHz
TIMER = "app::(anonymous namespace)::timer_isr"
NESTED = "app::(anonymous namespace)::nested_isr"
SETPOINT = "app::(anonymous namespace)::setpoint"
PEAK = 10000  # micrometres
# A debugger's watchpoint on the most capable comparator, 3, which the system
# reset keeps: a write to the flash's last word, which never comes, raises a
# debug event (MATCH 0b0101, ACTION 0b01, 4 bytes).
WATCHPOINT_COMPARATOR = 3
WATCHPOINT_ADDRESS = 0x081FFFFC
WATCHPOINT_FUNCTION = 0x815
WATCHPOINT = ["b32", hex(0xE0001020 + 16 * WATCHPOINT_COMPARATOR),
              hex(WATCHPOINT_ADDRESS), "0", hex(WATCHPOINT_FUNCTION)]


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


def check_setpoints(host: str, capture: str, output: str) -> list[str]:
    """The half sine's excursion, decoded and bounded by kymo check."""
    positions = [int(dict(fields)["position"]) for _, name, fields in parse(output) if name == SETPOINT]
    if not positions:
        return ["no setpoint recorded"]
    low, high = min(positions), max(positions)
    print(f"{len(positions)} setpoints, from {low} to {high}")
    errors = [] if (low, high) == (0, PEAK) else [f"the setpoints run from {low} to {high}, not 0 to {PEAK}"]

    def rule(stat: str, **bounds) -> dict:
        return {"subject": SETPOINT, "metric": "value", "detail": "position", "stat": stat, **bounds}

    for rules, code, label in (([rule("min", min=low), rule("max", max=high),
                                 rule("count", min=len(positions), max=len(positions))], 0, "at the excursion"),
                               ([rule("min", min=low + 1)], 3, "min one above"),
                               ([rule("max", max=high - 1)], 3, "max one below")):
        with tempfile.TemporaryDirectory() as folder:
            path = os.path.join(folder, "rules.json")
            with open(path, "w", encoding="utf-8") as file:
                json.dump({"rules": rules}, file)
            done = subprocess.run([host, "check", capture, path], capture_output=True, text=True, encoding="utf-8")
        if done.returncode != code:
            errors.append(f"check {label}: exit {done.returncode}, not {code}\n{done.stdout}{done.stderr}")
    return errors


def read_words(probe_rs: str, probe: str, address: int, count: int) -> list[int]:
    """count words from the running board, through the debug port."""
    done = subprocess.run([probe_rs, "read", "--chip", CHIP, "--probe", probe, "b32", hex(address), str(count)],
                          capture_output=True, text=True, encoding="utf-8", check=True)
    return [int(word, 16) for line in done.stdout.splitlines() if ":" in line for word in line.split(":")[1].split()]


def check_dwt(probe_rs: str, probe: str, nm: str, image: str) -> list[str]:
    """What kymo::dwt::start(), anchor() and watch() configured, read from the registers."""
    symbols = {}
    for line in subprocess.run([nm, "-C", image], capture_output=True, text=True, encoding="utf-8",
                               check=True).stdout.splitlines():
        address, _, name = line.split(" ", 2)
        symbols[name] = int(address, 16)
    anchor = symbols["trace_ring"] + 12  # write_index, lapped, then slot 0's header: its timestamp word
    position = symbols["app::(anonymous namespace)::g_position"]
    if read_words(probe_rs, probe, symbols["app::(anonymous namespace)::g_dwt_started"], 1) != [1]:
        return ["kymo::dwt::anchor() or watch() failed, or anchor() took a comparator before start()"]
    words = read_words(probe_rs, probe, 0xE0001020, 16)
    comparators = [(words[4 * n], words[4 * n + 2]) for n in range(4)]
    # A data value on write (MATCH 0b1101, ACTION 0b10) of 4 bytes (DATAVSIZE 2).
    sent = {comp: n for n, (comp, function) in enumerate(comparators) if function & 0xC3F == 0x82D}
    errors = [f"no comparator sends writes to {name} at {address:#x}"
              for name, address in (("the anchor", anchor), ("g_position", position)) if address not in sent]
    watchpoint = comparators[WATCHPOINT_COMPARATOR]
    if (watchpoint[0], watchpoint[1] & 0xFFF) != (WATCHPOINT_ADDRESS, WATCHPOINT_FUNCTION):
        errors.append(f"the debugger's watchpoint is gone: {watchpoint[0]:#x}, {watchpoint[1]:#x}")
    if not errors:
        # start() freed what the run before the reset left, and kept the
        # watchpoint: the anchor took, of the others, the one with the fewest
        # capabilities, the highest of those.
        ones = [bin(function >> 27).count("1") for _, function in comparators]
        free = [n for n in range(4) if n != WATCHPOINT_COMPARATOR]
        least = max(n for n in free if ones[n] == min(ones[m] for m in free))
        print(f"DWT: anchor on comparator {sent[anchor]}, g_position on {sent[position]}; capabilities {ones}")
        if sent[anchor] != least:
            errors.append(f"the anchor is on comparator {sent[anchor]}, not {least}, the least capable")
    itm_tcr = read_words(probe_rs, probe, 0xE0000E80, 1)[0]
    acpr, = read_words(probe_rs, probe, 0xE0040010, 1)
    sppr, = read_words(probe_rs, probe, 0xE00400F0, 1)
    ffcr, = read_words(probe_rs, probe, 0xE0040304, 1)
    # ITMENA, TSENA, TXENA; 8 MHz / 8 MBd; NRZ; the formatter off.
    if (itm_tcr & 0xB, acpr, sppr, ffcr) != (0xB, 0, 2, 0x100):
        errors.append(f"ITM_TCR {itm_tcr:#x}, ACPR {acpr}, SPPR {sppr}, FFCR {ffcr:#x}: not SWO at 8 MBd")
    dwt_ctrl, = read_words(probe_rs, probe, 0xE0001000, 1)
    if dwt_ctrl & (1 << 16) == 0:
        errors.append(f"DWT_CTRL {dwt_ctrl:#x}: kymo::dwt::exceptions() left EXCTRCENA clear")
    # CYCEVTENA, CYCTAP on bit 10, POSTPRESET 15: a tick every 1024 x 16 cycles.
    if dwt_ctrl & TICK_FIELDS != TICK_FIELDS:
        errors.append(f"DWT_CTRL {dwt_ctrl:#x}: kymo::dwt::ticks() did not set a tick every 16384 cycles")
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
    parser.add_argument("--nm", required=True)
    parser.add_argument("--seconds", type=int, default=30)
    args = parser.parse_args()

    probe = f"0483:374e:{args.probe_id}"
    for command in ("download", "write", "clear", "reset"):
        if command == "clear":
            # DWT_CTRL keeps EXCTRCENA and the tick across the reset: cleared,
            # so that only the image's exceptions() and ticks() set them.
            dwt_ctrl, = read_words(args.probe_rs, probe, 0xE0001000, 1)
            cleared = dwt_ctrl & ~((1 << 16) | TICK_FIELDS | (0xF << 5))
            arguments = ["write", "--probe", probe, "--chip", CHIP, "b32", "0xE0001000", hex(cleared)]
        else:
            arguments = [command, "--probe", probe, "--chip", CHIP] + {
                "download": [args.image], "write": WATCHPOINT, "reset": []}[command]
        flashed = subprocess.run([args.probe_rs] + arguments, capture_output=True, text=True, encoding="utf-8")
        if flashed.returncode != 0:
            print(flashed.stderr)
            print(f"probe-rs {command} failed")
            return 1
    dwt_errors = check_dwt(args.probe_rs, probe, args.nm, args.image)
    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "board.kcap")
        record = subprocess.run(
            [args.host, "record", "--probe", "probe-rs", "--probe-id", probe, "--chip", CHIP,
             "--elf", args.image, "--duration", str(args.seconds), "--out", capture],
            stdin=subprocess.DEVNULL, capture_output=True, text=True, encoding="utf-8")
        print(record.stderr.strip())
        if record.returncode != 0:
            print("kymo record failed")
            return 1
        decode = subprocess.run([args.host, "decode", capture], capture_output=True, text=True, encoding="utf-8")
        if decode.returncode != 0:
            print(decode.stderr)
            print("kymo decode failed")
            return 1
        errors = (dwt_errors + check(decode.stdout) + check_board(decode.stdout)
                  + check_setpoints(args.host, capture, decode.stdout)
                  + check_stats(args.host, args.image, capture, {SAMPLING: None})
                  + check_rules(args.host, args.image, capture))
        lost = re.search(r"(\d+) slots lost", record.stderr)
        if lost is None or int(lost.group(1)) != 0:
            errors.append(f"slots lost while recording: {lost.group(1) if lost else 'no summary'}")
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
