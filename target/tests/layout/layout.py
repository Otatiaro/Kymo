#!/usr/bin/env python3
"""Compare the event layouts the host reads from the DWARF with the target's.

    layout.py --qemu Q --machine M --image I --host KYMO

Runs the image under QEMU, which reports each event's id, name, size and
fields as the compiler laid them out (layout.cpp), then `kymo events` on the
same image. Both must list the same events, each with the same id, size and
fields, in the same order.
"""

import argparse
import subprocess
import sys

TIMEOUT_SECONDS = 60


def catalogue(text: str) -> list[str]:
    return [line.rstrip() for line in text.splitlines() if line.lstrip().startswith(("event ", "field "))]


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

    host = subprocess.run([args.host, "events", args.image],
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=TIMEOUT_SECONDS)
    if host.returncode != 0:
        print(host.stdout)
        print("kymo events failed")
        return 1

    expected = sorted_by_id(catalogue(target.stdout))
    found = catalogue(host.stdout)
    if expected == found:
        print("\n".join(found))
        print(f"{sum(line.startswith('event ') for line in found)} events: the DWARF agrees with the target")
        return 0

    print("target says:")
    print("\n".join(expected))
    print("kymo events says:")
    print("\n".join(found))
    for line in expected:
        if line not in found:
            print(f"missing or different: {line}")
    for line in found:
        if line not in expected:
            print(f"unexpected: {line}")
    return 1


def sorted_by_id(lines: list[str]) -> list[str]:
    """The target reports in source order; the host prints by id."""
    events: list[list[str]] = []
    for line in lines:
        if line.startswith("event "):
            events.append([line])
        else:
            events[-1].append(line)
    events.sort(key=lambda event: int(event[0].split()[1]))
    return [line for event in events for line in event]


if __name__ == "__main__":
    sys.exit(main())
