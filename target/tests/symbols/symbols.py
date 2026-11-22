#!/usr/bin/env python3
"""Record a running image through QEMU's monitor, and check that pointer
fields are named from the ELF.

    symbols.py --qemu Q --machine M --image I --host KYMO

Runs the image under QEMU, records it with `kymo record --probe qemu` for a
few seconds, then checks that every event prints one of the values its image
wrote (symbols.cpp), named as the host names them: a task control block by
its task, a member mutex by its object and offset, a handler by its function,
and an address in no symbol in hexadecimal; and that every value was seen.
"""

import argparse
import os
import subprocess
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "record"))
from record import TIMEOUT_SECONDS, free_port, wait_for  # noqa: E402

RECORD_SECONDS = "2"

EXPECTED = {
    "task_started": [
        {"task": "app::first_task"},
        {"task": "app::second_task"},
    ],
    "mutex_taken": [
        {"mutex": "app::lock", "owner": "app::first_task"},
        {"mutex": "app::uart+0x4", "owner": "app::second_task"},
    ],
    "notify": [{"condition": "app::ready"}],
    "isr_enter": [{"handler": "app::timer_handler"}],
    "unnamed": [{"outside": "0x40000000", "null": "0x00000000"}],
}


def check(output: str) -> list[str]:
    errors = []
    seen = set()
    for line in output.splitlines():
        fields = line.split()
        if fields[0] != "event":
            continue
        name = fields[4]
        values = dict(field.split("=", 1) for field in fields[5:])
        if values not in EXPECTED.get(name, []):
            errors.append(f"{line}: expected one of {EXPECTED.get(name)}")
            continue
        seen.add((name, EXPECTED[name].index(values)))
    for name, variants in EXPECTED.items():
        for n, values in enumerate(variants):
            if (name, n) not in seen:
                errors.append(f"never recorded: {name} {values}")
    print(f"{len(seen)} of {sum(map(len, EXPECTED.values()))} values seen")
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
    try:
        wait_for(port)
        record = subprocess.run(
            [args.host, "record", "--probe", "qemu", "--probe-id", f"127.0.0.1:{port}",
             "--elf", args.image, "--duration", RECORD_SECONDS],
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
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
