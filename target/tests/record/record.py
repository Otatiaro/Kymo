#!/usr/bin/env python3
"""Record a running image through QEMU's monitor, and check what was recorded.

    record.py --qemu Q --machine M --image I --host KYMO

Runs the image under QEMU with its monitor on a TCP port, records it with
`kymo record --probe qemu` for a few seconds into a capture file, then
checks the output (record.cpp):

- every index from where recording started to where it stopped is in one
  event or one loss, in order, never both, never twice, and a loss is never
  followed by another;
- every event holds the values its image wrote for its index;
- the recording went through losses, every type, and events whose slots run
  into the guard zone;
- `kymo decode` of the capture prints exactly what `kymo record` printed:
  they may differ only by the seconds, which are all 0 here (QEMU's
  timestamps all read 0) at the frequency given and kept in the capture.

Which indexes are lost depends on how QEMU and the host interleave, so the
check does not predict them: it checks that the output accounts for each.
"""

import argparse
import os
import socket
import subprocess
import sys
import tempfile
import time

RECORD_SECONDS = "2"
CLOCK_HZ = "64000000"  # given, so that the capture carries it to kymo decode
TIMEOUT_SECONDS = 60
SLOTS = 16  # KYMO_RING_SLOTS of the test images
TYPES = {"one": 1, "two": 2, "four": 4}  # slots each type takes


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def wait_for(port: int) -> None:
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while True:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
            return
        except OSError:
            if time.monotonic() > deadline:
                raise
            time.sleep(0.05)


def expected_fields(name: str, index: int) -> dict[str, str]:
    if name == "one":
        return {"index": str(index)}
    if name == "two":
        return {"index": str(index), "words": f"[{index + 1},{index + 2},{index + 3}]"}
    words = ",".join(str((index * 10 + k) & 0xFFFFFFFF) for k in range(10))
    return {"index": str(index), "words": f"[{words}]"}


def check(output: str) -> list[str]:
    errors = []
    lines = output.splitlines()
    if len(lines) < 2 or not lines[0].startswith("record ") or not lines[-1].startswith("end "):
        return ["the output does not run from a record line to an end line"]
    expected = int(lines[0].split()[1])
    events = dict.fromkeys(TYPES, 0)
    losses = guard = 0
    previous = None
    for line in lines[1:-1]:
        fields = line.split()
        if fields[0] == "lost":
            start, end = int(fields[1]), int(fields[2])
            if start != expected or end <= start:
                errors.append(f"{line}: expected a loss from {expected}")
            if previous == "lost":
                errors.append(f"{line}: a loss right after a loss")
            previous = "lost"
            losses += 1
            expected = end
            continue
        if fields[0] == "suspect":
            errors.append(f"{line}: no writer of the image is held for a lap")
            continue
        previous = "event"
        index, name = int(fields[1]), fields[4]
        if fields[0] != "event" or name not in TYPES:
            errors.append(f"{line}: not an event of the image")
            continue
        # QEMU has no cycle counter: no cycles, so 0 s at the frequency given.
        if fields[2:4] != ["0", "0.000000000"]:
            errors.append(f"{line}: expected 0 cycles at 0 s")
        values = dict(field.split("=", 1) for field in fields[5:])
        if index != expected:
            errors.append(f"{line}: expected index {expected}")
        if values != expected_fields(name, index):
            errors.append(f"{line}: expected {expected_fields(name, index)}")
        events[name] += 1
        guard += index % SLOTS + TYPES[name] > SLOTS
        expected = index + TYPES[name]
    end = int(lines[-1].split()[1])
    if expected != end:
        errors.append(f"the events and losses reach {expected}, the recording ends at {end}")
    print(f"events {events}, {losses} losses, {guard} through the guard zone")
    if losses == 0 or guard == 0 or 0 in events.values():
        errors.append("the recording did not go through losses, every type and the guard zone")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    args = parser.parse_args()

    port = free_port()
    target = subprocess.Popen(
        [args.qemu, "-machine", args.machine, "-kernel", args.image, "-nographic",
         "-semihosting-config", "enable=on,target=native", "-monitor", "none", "-serial", "none",
         "-qmp", f"tcp:127.0.0.1:{port},server=on,wait=off"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "record.kcap")
        try:
            wait_for(port)
            record = subprocess.run(
                [args.host, "record", "--probe", "qemu", "--probe-id", f"127.0.0.1:{port}",
                 "--elf", args.image, "--duration", RECORD_SECONDS, "--out", capture,
                 "--clock", CLOCK_HZ],
                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=TIMEOUT_SECONDS)
        finally:
            target.kill()
            target.wait()
        print(record.stderr.strip())
        if record.returncode != 0:
            print(record.stdout[-2000:])
            print("kymo record failed")
            return 1
        errors = check(record.stdout)
        replay = subprocess.run([args.host, "decode", args.image, capture],
                                capture_output=True, text=True, timeout=TIMEOUT_SECONDS)
        if replay.returncode != 0:
            errors.append(f"kymo decode of the capture failed: {replay.stderr.strip()}")
        elif replay.stdout != record.stdout:
            errors.append("kymo decode of the capture differs from what kymo record printed")
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
