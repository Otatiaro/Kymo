#!/usr/bin/env python3
"""Compare what the host writes for events whose types declare a text with
what the image says it must write.

    text.py --qemu Q --machine M --image I --host KYMO [--bad]

Runs the image under QEMU, which reports each type's text, emits events and
reports the line the host must print for each, then a copy of the ring
(text.cpp). `kymo events` must list each type's text, and `kymo decode` on
the copy must print each event's line.

With --bad, the image holds a type whose text names a field it does not
have: `kymo events` must refuse it, and say which type and which field.
"""

import argparse
import os
import subprocess
import sys
import tempfile

TIMEOUT_SECONDS = 60
BAD_TEXT = "wrong: kymo_text: {y}: the event has no field y"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    parser.add_argument("--bad", action="store_true")
    args = parser.parse_args()

    if args.bad:
        host = subprocess.run([args.host, "events", args.image], stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True, timeout=TIMEOUT_SECONDS)
        if host.returncode != 0 and BAD_TEXT in host.stdout:
            print(host.stdout.strip())
            return 0
        print(host.stdout)
        print(f"kymo events must fail with: {BAD_TEXT}")
        return 1

    target = subprocess.run(
        [args.qemu, "-machine", args.machine, "-kernel", args.image, "-nographic",
         "-semihosting-config", "enable=on,target=native", "-monitor", "none", "-serial", "none"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=TIMEOUT_SECONDS)
    if target.returncode != 0 or "RESULT: PASS" not in target.stdout:
        print(target.stdout)
        print("the image did not run to its end")
        return 1

    texts = {}
    lines = []
    write_index = 0
    ring = b""
    for line in target.stdout.splitlines():
        if line.startswith("text "):
            _, name, template = line.split(" ", 2)
            texts[name] = template
        elif line.startswith("emitted "):
            _, index, expected = line.split(" ", 2)
            lines.append(expected)
            write_index = int(index) + 1
            if expected.startswith(f"event {index}-"):
                write_index = int(expected.split()[1].split("-")[1]) + 1
        elif line.startswith("ring "):
            ring = bytes.fromhex(line.split()[1])
    if not texts or not lines or not ring:
        print(target.stdout)
        print("the image did not report its texts, its events and the ring")
        return 1

    failed = False
    events = subprocess.run([args.host, "events", args.image], stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, timeout=TIMEOUT_SECONDS)
    listed = {}
    current = None
    for line in events.stdout.splitlines():
        if line.startswith("event "):
            current = line.split()[2]
        elif line.startswith("  text ") and current is not None:
            listed[current] = line[len("  text "):]
    if events.returncode != 0 or listed != texts:
        failed = True
        print(events.stdout)
        print(f"kymo events lists the texts {listed}, the image declares {texts}")

    expected = [f"write_index {write_index}"] + lines
    with tempfile.TemporaryDirectory() as directory:
        dump = os.path.join(directory, "ring.bin")
        with open(dump, "wb") as file:
            file.write(ring)
        host = subprocess.run([args.host, "decode", args.image, dump], stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True, timeout=TIMEOUT_SECONDS)
    found = host.stdout.splitlines()
    if host.returncode == 0 and found == expected:
        print("\n".join(found))
        print(f"{len(lines)} events, written as their texts say")
    else:
        failed = True
        print("the image says:")
        print("\n".join(expected))
        print("kymo decode says:")
        print("\n".join(found))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
