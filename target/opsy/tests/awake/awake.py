"""Checks that a traced image never sleeps: no WFI in its code.

The adapter's idle_wait is a NOP. On some parts the probe reads the system
bus as zeros while the core sleeps, the ring included, so a traced program
that reached a WFI would show the host a ring read as zeros.
"""

import argparse
import re
import subprocess
import sys


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--objdump", required=True)
    parser.add_argument("--image", required=True)
    arguments = parser.parse_args()
    listing = subprocess.run(
        [arguments.objdump, "-d", "-C", arguments.image], check=True, capture_output=True, text=True
    ).stdout
    function = None
    sleeping = []
    for line in listing.splitlines():
        start = re.match(r"^[0-9a-f]+ <(.*)>:$", line)
        if start:
            function = start.group(1)
        elif re.search(r"\twfi(\.n|\.w)?\b", line):
            sleeping.append(f"{function}: {line.strip()}")
    if not any(re.match(r"^[0-9a-f]+ <", line) for line in listing.splitlines()):
        print(f"error: no code disassembled from {arguments.image}")
        return 1
    if sleeping:
        print("error: the traced image sleeps, the probe would read its ring as zeros:")
        print("\n".join(sleeping))
        return 1
    print(f"{arguments.image}: no WFI")
    return 0


if __name__ == "__main__":
    sys.exit(main())
