#!/usr/bin/env python3
"""Check that each event reaches the ring of its family, and that the host
reads a ring of a two-ring image only by its name.

    channels.py --qemu Q --machine M --image I --host KYMO

Runs the image (channels.cpp), which reports each event with the ring it is
routed to, then copies of both rings. Then:

- `kymo decode --channel <ring>` on each copy prints exactly the events
  routed to that ring;
- without --channel, `kymo decode` and `kymo record` fail, naming both
  rings; with a ring the image does not have, they fail naming it;
- a capture of app::ring decodes without --channel: the capture names its
  ring. Its event lies past the 16th slot, where the lap in a slot header
  depends on the ring's size, so it decodes with app::ring's 32 slots and not
  with os::ring's 16;
- a capture naming os::ring, or a ring the image does not have, fails; and
  --channel on a capture is refused.
"""

import argparse
import os
import struct
import subprocess
import sys
import tempfile

TIMEOUT_SECONDS = 60
RINGS = {"os::ring": 16, "app::ring": 32}
SLOT_BYTES = 16


def capture(image: str, channel: str, index: int, slots: bytes, write_index: int) -> bytes:
    """A capture of one event, in the format of host/src/capture.rs, naming
    the ELF `image` by its absolute path."""
    name = channel.encode()
    elf = os.path.abspath(image).encode()
    header = (b"KYMOCAP\0" + struct.pack("<IIQH", 1, index, 0, len(name)) + name
              + b"\x02" + struct.pack("<H", len(elf)) + elf)
    passed = b"\x01" + struct.pack("<QI", 0, write_index)
    event = b"\x02" + struct.pack("<IB", index, len(slots) // SLOT_BYTES) + slots
    end = b"\x04" + struct.pack("<I", write_index)
    return header + passed + event + end


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

    lines: dict[str, list[tuple[int, str]]] = {ring: [] for ring in RINGS}
    copies: dict[str, bytes] = {}
    slots_offset = None
    for line in target.stdout.splitlines():
        if line.startswith("emitted "):
            _, ring, index, slots, rest = line.split(" ", 4)
            last = int(index) + int(slots) - 1
            shown = index if slots == "1" else f"{index}-{last}"
            lines[ring].append((int(index), f"event {shown} 0 - {rest}"))
        elif line.startswith("slots "):
            slots_offset = int(line.split()[1])
        elif line.startswith("ring "):
            _, ring, data = line.split()
            copies[ring] = bytes.fromhex(data)
    if set(copies) != set(RINGS) or slots_offset is None:
        print(target.stdout)
        print("the image did not report both rings")
        return 1

    errors = []

    def host(*arguments: str) -> subprocess.CompletedProcess:
        return subprocess.run([args.host, *arguments], capture_output=True, text=True, timeout=TIMEOUT_SECONDS)

    def fails_with(what: str, result: subprocess.CompletedProcess, *texts: str) -> None:
        if result.returncode == 0 or not all(text in result.stderr for text in texts):
            errors.append(f"{what}: expected an error naming {', '.join(texts)}; got {result.returncode}, "
                          f"{result.stderr.strip() or result.stdout.strip()}")

    with tempfile.TemporaryDirectory() as directory:
        def file(name: str, data: bytes) -> str:
            path = os.path.join(directory, name)
            with open(path, "wb") as out:
                out.write(data)
            return path

        dumps = {ring: file(f"{ring.replace('::', '_')}.bin", data) for ring, data in copies.items()}
        for ring, dump in dumps.items():
            write_index = struct.unpack_from("<I", copies[ring])[0]
            expected = [f"write_index {write_index}"] + [line for _, line in lines[ring]]
            found = host("decode", dump, "--elf", args.image, "--channel", ring)
            if found.returncode != 0 or found.stdout.splitlines() != expected:
                errors.append(f"{ring}: expected\n" + "\n".join(expected) + "\nkymo decode says\n"
                              + (found.stdout + found.stderr).strip())
            else:
                print(f"{ring}: {len(expected) - 1} events, decoded as routed")

        fails_with("decode without --channel", host("decode", dumps["os::ring"], "--elf", args.image),
                   "several rings", *RINGS)
        fails_with("decode of an absent ring", host("decode", dumps["os::ring"], "--elf", args.image, "--channel", "nowhere::ring"),
                   "no ring nowhere::ring")
        # The ring is chosen before the probe is opened: no target is needed.
        fails_with("record without --channel",
                   host("record", "--probe", "qemu", "--probe-id", "127.0.0.1:9", "--elf", args.image),
                   "several rings", *RINGS)

        # The last mark of app::ring: its index is past the 16th slot.
        index, line = [entry for entry in lines["app::ring"] if " mark " in entry[1]][-1]
        app = copies["app::ring"]
        write_index = struct.unpack_from("<I", app)[0]
        if index < RINGS["os::ring"] or index >= RINGS["app::ring"]:
            errors.append(f"the last mark is at index {index}: change the sequence in channels.cpp")
        start = slots_offset + (index % RINGS["app::ring"]) * SLOT_BYTES
        slots = app[start:start + SLOT_BYTES]
        replay = host("decode", file("app.kcap", capture(args.image, "app::ring", index, slots, write_index)))
        if replay.returncode != 0 or line not in replay.stdout.splitlines():
            errors.append(f"capture of app::ring: expected {line}; kymo decode says "
                          + (replay.stdout + replay.stderr).strip())
        else:
            print(f"capture of app::ring, decoded without --channel: {line}")
        fails_with("a capture of app::ring's slots naming os::ring",
                   host("decode", file("os.kcap", capture(args.image, "os::ring", index, slots, write_index))),
                   "do not hold it whole")
        fails_with("a capture naming an absent ring",
                   host("decode", file("absent.kcap", capture(args.image, "nowhere::ring", index, slots, write_index))),
                   "no ring nowhere::ring")
        fails_with("--channel on a capture",
                   host("decode", file("app2.kcap", capture(args.image, "app::ring", index, slots, write_index)),
                        "--channel", "app::ring"),
                   "--channel names the ring of a copy")

    for error in errors:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
