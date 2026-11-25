#!/usr/bin/env python3
"""Compare what the host decodes from copies of the ring with what was written.

    decode.py --qemu Q --machine M --image I --host KYMO

Runs the image under QEMU, which emits events, reports each with the line the
host must print for it, then reports two copies of the ring: one taken with
nothing writing, one taken slot by slot while events keep coming (decode.cpp).

Which events a copy holds is worked out here from the ring's rule (ring.hpp):
those the write index of the copy has not gone a full ring past, whose every
slot was copied before a later event took it. `kymo decode` on each copy must
print exactly those, oldest first, and the write index of the copy.
"""

import argparse
import os
import subprocess
import sys
import tempfile

TIMEOUT_SECONDS = 60
SLOTS = 16  # ring_slots of the test images (test_trace.hpp)


class Copy:
    def __init__(self, write_index: int):
        self.write_index = write_index
        self.owner_when_copied: dict[int, int | None] = {}
        self.bytes = b""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    args = parser.parse_args()

    target = subprocess.run(
        [args.qemu, "-machine", args.machine, "-kernel", args.image, "-nographic",
         "-semihosting-config", "enable=on,target=native", "-monitor", "none", "-serial", "none"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=TIMEOUT_SECONDS)
    if target.returncode != 0 or "RESULT: PASS" not in target.stdout:
        print(target.stdout)
        print("the image did not run to its end")
        return 1

    owner: dict[int, int] = {}  # physical slot -> index of the event that last took it
    lines: dict[int, str] = {}
    counts: dict[int, int] = {}
    write_index = 0
    copies: dict[str, Copy] = {}
    live = None
    for line in target.stdout.splitlines():
        if line.startswith("emitted "):
            _, index, count, expected = line.split(" ", 3)
            index, count = int(index), int(count)
            for slot in range(count):
                owner[(index + slot) % SLOTS] = index
            lines[index] = expected
            counts[index] = count
            write_index = index + count
        elif line.startswith("ring "):
            copies["stopped"] = Copy(write_index)
            copies["stopped"].owner_when_copied = dict(owner)
            copies["stopped"].bytes = bytes.fromhex(line.split()[1])
        elif line == "copy":
            live = copies["live"] = Copy(write_index)
        elif line.startswith("copied "):
            slot = int(line.split()[1])
            live.owner_when_copied[slot] = owner.get(slot)
        elif line.startswith("live "):
            live.bytes = bytes.fromhex(line.split()[1])
    if set(copies) != {"stopped", "live"} or not all(copy.bytes for copy in copies.values()):
        print(target.stdout)
        print("the image did not report both copies")
        return 1

    held = {}
    for name, copy in copies.items():
        window = [index for index in sorted(lines) if index < copy.write_index and copy.write_index - index <= SLOTS]
        held[name] = [index for index in window
                      if all(copy.owner_when_copied.get((index + slot) % SLOTS) == index
                             for slot in range(counts[index]))]
        if name == "stopped" and held[name] != window:
            print(f"events within a ring of the write index, yet overwritten: {sorted(set(window) - set(held[name]))}")
            return 1

    # What the copies must hold for the comparison to mean something.
    live = copies["live"]
    window = [index for index in lines if index < live.write_index and live.write_index - index <= SLOTS]
    kept = held["stopped"] + held["live"]
    coverage = {
        "an event across the end of the ring": any(index % SLOTS + counts[index] > SLOTS for index in kept),
        "an overwritten event": len(held["stopped"]) < len(lines),
        "an enumeration value by name": any(" grade=low" in lines[index] or " grade=high" in lines[index]
                                            or " grade=mid" in lines[index] for index in kept),
        "an enumeration value no enumerator names": any(" grade=5 " in lines[index] for index in kept),
        "a live copy with an event whose first slot was taken before it was copied":
            any(live.owner_when_copied[index % SLOTS] != index for index in window),
        "a live copy with an event whose later slot was taken after its first was copied":
            any(live.owner_when_copied[index % SLOTS] == index and index not in held["live"] for index in window),
    }
    for what, covered in coverage.items():
        if not covered:
            print(f"no {what}: change the sequence in decode.cpp")
            return 1

    failed = False
    for name, copy in copies.items():
        expected = [f"write_index {copy.write_index}"] + [lines[index] for index in held[name]]
        with tempfile.TemporaryDirectory() as directory:
            dump = os.path.join(directory, f"{name}.bin")
            with open(dump, "wb") as file:
                file.write(copy.bytes)
            host = subprocess.run([args.host, "decode", dump, "--elf", args.image], stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, text=True, timeout=TIMEOUT_SECONDS)
        found = host.stdout.splitlines()
        if host.returncode == 0 and expected == found:
            print(f"{name} copy:")
            print("\n".join(found))
            print(f"{len(held[name])} of {len(lines)} events, decoded as written")
            continue
        failed = True
        print(f"{name} copy, written and still whole:")
        print("\n".join(expected))
        print("kymo decode says:")
        print("\n".join(found))
        for line in expected:
            if line not in found:
                print(f"missing or different: {line}")
        for line in found:
            if line not in expected:
                print(f"unexpected: {line}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
