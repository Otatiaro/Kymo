#!/usr/bin/env python3
"""Check the interrupts the host names from an image's DWARF.

    interrupts.py --host KYMO --image I --convention C [--svd S]

`kymo events` lists the device interrupts the DWARF's IRQn_Type names
(interrupts.cpp), then those the SVD file names and the DWARF does not. The
convention says what the image declares: svdgenerator, cmsis, or both, which
names none and warns.
"""

import argparse
import subprocess
import sys

TIMEOUT_SECONDS = 60

EXPECTED = {
    "svdgenerator": ["interrupt 16 WWDG", "interrupt 61 TIM2"],
    "cmsis": ["interrupt 16 WWDG", "interrupt 44 TIM2"],
    "both": [],
}

# What device.svd adds: TIM3 at 46. Its TIMER2 at 45 is TIM2 in the DWARF.
SVD = {
    "svdgenerator": ["interrupt 16 WWDG", "interrupt 61 TIM2", "interrupt 62 TIM3"],
    "cmsis": ["interrupt 16 WWDG", "interrupt 44 TIM2", "interrupt 61 TIMER2", "interrupt 62 TIM3"],
    "both": ["interrupt 61 TIMER2", "interrupt 62 TIM3"],
}

WARNING = "different IRQn_Type"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--convention", required=True, choices=EXPECTED)
    parser.add_argument("--svd")
    args = parser.parse_args()

    command = [args.host, "events", args.image]
    expected = EXPECTED[args.convention]
    if args.svd:
        command += ["--svd", args.svd]
        expected = SVD[args.convention]
    host = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                          timeout=TIMEOUT_SECONDS)
    if host.returncode != 0:
        print(host.stdout, host.stderr)
        print("kymo events failed")
        return 1

    found = [line.rstrip() for line in host.stdout.splitlines() if line.startswith("interrupt ")]
    warned = WARNING in host.stderr
    should_warn = args.convention == "both"
    if found == expected and warned == should_warn:
        print("\n".join(found))
        print(f"{len(found)} interrupts named, as expected" + (", with the warning" if warned else ""))
        return 0

    print("expected:")
    print("\n".join(expected))
    print("kymo events says:")
    print("\n".join(found))
    if warned != should_warn:
        print(f"warning expected: {should_warn}, given: {warned}")
        print(host.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
