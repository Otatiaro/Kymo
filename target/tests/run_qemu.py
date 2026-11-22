#!/usr/bin/env python3
"""Run a test image under QEMU.

    run_qemu.py --qemu Q --machine M --image I [--icount-shift N]

The image reports its own verdict through semihosting, as a line
"RESULT: PASS" or "RESULT: FAIL"; anything else, a hang or a crash included,
is a failure. With --icount-shift, QEMU runs deterministically, each
instruction taking 2^N ns of virtual time.
"""

import argparse
import subprocess
import sys

TIMEOUT_SECONDS = 300


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--icount-shift", type=int)
    args = parser.parse_args()

    command = [
        args.qemu, "-machine", args.machine, "-kernel", args.image, "-nographic",
        "-semihosting-config", "enable=on,target=native", "-monitor", "none", "-serial", "none",
    ]
    if args.icount_shift is not None:
        command += ["-icount", f"shift={args.icount_shift},sleep=off"]

    try:
        qemu = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              text=True, timeout=TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired as expired:
        print(expired.output or "")
        print(f"timed out after {TIMEOUT_SECONDS} s")
        return 1

    print(qemu.stdout)
    return 0 if "RESULT: PASS" in qemu.stdout and qemu.returncode == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
