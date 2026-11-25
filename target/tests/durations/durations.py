#!/usr/bin/env python3
"""Compare how the host reads and pairs the application's durations with
what the image says it must find.

    durations.py --qemu Q --machine M --image I --host KYMO

Runs the image under QEMU, which reports the line of each event, each
duration, each one still open at the end and each event that contradicts
the pairing (durations.cpp). Then records the same image, and on the
capture:
- `kymo decode` must print each event's line;
- `kymo stats --json` must give, for each type of duration, its durations
  and the one cut, and count the contradictions;
- `kymo check` must hold a duration's max at its value and fail it one
  cycle below, and bound the contradictions.
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "record"))
from record import record_to_exit  # noqa: E402

TIMEOUT_SECONDS = 60
WORST = 20


def run(args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(args, capture_output=True, text=True, encoding="utf-8", timeout=TIMEOUT_SECONDS)


def expected_of(stdout: str) -> tuple[list[str], dict[str, list[tuple[int, int]]], dict[str, list[int]], int]:
    """The event lines, the durations by type as (value, begin), the begins
    cut by type, and the count of contradictions the image reports."""
    lines, durations, cut, contradictions = [], {}, {}, 0
    for line in stdout.splitlines():
        words = line.split()
        if line.startswith("event "):
            lines.append(line)
        elif line.startswith("duration "):
            durations.setdefault(words[1], []).append((int(words[3]), int(words[2])))
        elif line.startswith("cut "):
            cut.setdefault(words[1], []).append(int(words[2]))
            durations.setdefault(words[1], [])
        elif line.startswith("contradiction "):
            contradictions += 1
    return lines, durations, cut, contradictions


def decoded_lines(stdout: str) -> list[str]:
    """kymo decode's event lines, less their seconds."""
    lines = []
    for line in stdout.splitlines():
        if line.startswith("event "):
            words = line.split(" ", 4)
            lines.append(" ".join(words[:3] + words[4:]))
    return lines


def compare_stats(report: dict, durations: dict, cut: dict, contradictions: int) -> list[str]:
    errors = []
    if report["contradictions"] != contradictions:
        errors.append(f"stats: {report['contradictions']} contradictions, the image reports {contradictions}")
    groups = {group["subject"]: group for group in report["groups"] if group["kind"] == "duration"}
    if sorted(groups) != sorted(durations):
        errors.append(f"stats: durations of {sorted(groups)}, the image has {sorted(durations)}")
    for subject, values in durations.items():
        group = groups.get(subject)
        if group is None:
            continue
        if group["metric"] != "duration" or group["detail"] is not None:
            errors.append(f"stats {subject}: metric {group['metric']}, detail {group['detail']}")
        if group["count"] != len(values) or group["cut"] != len(cut.get(subject, [])):
            errors.append(f"stats {subject}: {group['count']} counted and {group['cut']} cut, "
                          f"the image has {len(values)} and {len(cut.get(subject, []))}")
        distribution = group["distribution"]
        found = [] if distribution is None else [(point["value"], point["index"]) for point in distribution["worst"]]
        wanted = sorted(values, key=lambda value: (-value[0], value[1]))
        if found != wanted:
            errors.append(f"stats {subject}: durations (cycles, begin) {found}, the image has {wanted}")
    return errors


def compare_check(host: str, image: str, capture: str, durations: dict, contradictions: int) -> list[str]:
    errors = []
    subject, values = max(((subject, values) for subject, values in durations.items() if values),
                          key=lambda item: len(item[1]))
    high = max(value for value, _ in values)

    def check(rules: list[dict], code: int, label: str) -> None:
        with tempfile.TemporaryDirectory() as folder:
            path = os.path.join(folder, "rules.json")
            with open(path, "w", encoding="utf-8") as file:
                json.dump({"rules": rules}, file)
            done = run([host, "check", capture, path])
        if done.returncode != code:
            errors.append(f"check {label}: exit {done.returncode}, not {code}\n{done.stdout}{done.stderr}")

    def rule(maximum: int) -> dict:
        return {"kind": "duration", "subject": subject, "metric": "duration", "stat": "max",
                "max": f"{maximum}cycles"}

    check([rule(high), {"capture": "contradictions", "min": contradictions, "max": contradictions}], 0, "at")
    check([rule(high - 1)], 3, f"{subject} max one cycle below")
    check([{"capture": "contradictions", "max": contradictions - 1}], 3, "contradictions below")
    return errors


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
    lines, durations, cut, contradictions = expected_of(target.stdout)
    if target.returncode != 0 or "RESULT: PASS" not in target.stdout or not lines or not durations:
        print(target.stdout)
        print("the image did not run to its end")
        return 1

    errors = []
    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "durations.kymo")
        recording = record_to_exit(args.qemu, args.machine, args.image, args.host, capture)
        if recording.returncode != 0 or recording.exit_status != 0:
            print(recording.stdout)
            print(recording.stderr)
            print("the recording did not reach the image's end")
            return 1
        decode = run([args.host, "decode", capture])
        found = decoded_lines(decode.stdout)
        if decode.returncode != 0 or found != lines:
            errors.append("the image says:\n" + "\n".join(lines) + "\nkymo decode says:\n" + "\n".join(found)
                          + decode.stderr)
        stats = run([args.host, "stats", capture, "--json", "--worst", str(WORST)])
        if stats.returncode != 0:
            errors.append(f"kymo stats failed: {stats.stderr.strip()}")
        else:
            errors += compare_stats(json.loads(stats.stdout), durations, cut, contradictions)
        errors += compare_check(args.host, args.image, capture, durations, contradictions)

    if errors:
        print("\n".join(errors))
        return 1
    print("\n".join(lines))
    print(f"{len(lines)} events; durations of {', '.join(sorted(durations))} paired as the image says, "
          f"{contradictions} contradictions")
    return 0


if __name__ == "__main__":
    sys.exit(main())
