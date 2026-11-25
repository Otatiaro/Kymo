#!/usr/bin/env python3
"""Compare how the host reads the numeric fields of application events as
curves with the values the image says it emitted.

    values.py --qemu Q --machine M --image I --host KYMO

Runs the image under QEMU, which reports the index of each event it emits
and the bits of each numeric field (values.cpp). Then records the same
image, and on the capture:
- `kymo stats --json` must give one value group per curve, no other, each
  with its count, NaN count, min and max (the earliest among equals), its
  percentiles of rank ceil(p n) and its mean, as computed here;
- `kymo stats` must write a line per curve;
- `kymo check` must hold bounds at the values and fail them past, and
  refuse a rule on a field that is no value.
"""

import argparse
import json
import math
import os
import struct
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "record"))
from record import record_to_exit  # noqa: E402

TIMEOUT_SECONDS = 60

READINGS = {
    "i8": lambda bits: struct.unpack("<b", struct.pack("<B", bits))[0],
    "i32": lambda bits: struct.unpack("<i", struct.pack("<I", bits))[0],
    "i64": lambda bits: struct.unpack("<q", struct.pack("<Q", bits))[0],
    "u16": lambda bits: bits,
    "u32": lambda bits: bits,
    "f32": lambda bits: struct.unpack("<f", struct.pack("<I", bits))[0],
    "f64": lambda bits: struct.unpack("<d", struct.pack("<Q", bits))[0],
}


def run(args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(args, capture_output=True, text=True, encoding="utf-8", timeout=TIMEOUT_SECONDS)


def expected_of(stdout: str) -> dict[str, list[tuple[float | int, int]]]:
    """The samples of each curve the image reports, as (value, index)."""
    curves, index = {}, None
    for line in stdout.splitlines():
        words = line.split()
        if line.startswith("emit "):
            index = int(words[1])
        elif line.startswith("value "):
            curve, reading, bits = words[1], words[2], int(words[3], 16)
            curves.setdefault(curve, []).append((READINGS[reading](bits), index))
    return curves


def rank(values: list, fraction: float):
    return values[max(math.ceil(fraction * len(values)), 1) - 1]


def same(found, wanted) -> bool:
    if isinstance(wanted, int) and not isinstance(wanted, bool):
        return isinstance(found, int) and found == wanted
    return isinstance(found, (int, float)) and math.isclose(found, wanted, rel_tol=1e-12, abs_tol=0.0)


def compare_stats(report: dict, curves: dict) -> list[str]:
    errors = []
    if report["losses"] != 0 or report["suspects"] != 0:
        errors.append(f"stats: {report['losses']} losses, {report['suspects']} suspects")
    groups = {f"{group['subject']}.{group['detail']}": group for group in report["groups"] if group["kind"] == "value"}
    if sorted(groups) != sorted(curves):
        errors.append(f"stats: curves {sorted(groups)}, the image has {sorted(curves)}")
    for curve, samples in curves.items():
        group = groups.get(curve)
        if group is None:
            continue
        counted = [(value, index) for value, index in samples if not (isinstance(value, float) and math.isnan(value))]
        nan = len(samples) - len(counted)
        if group["metric"] != "value" or group["count"] != len(counted) or group["nan"] != nan:
            errors.append(f"stats {curve}: metric {group['metric']}, {group['count']} counted and {group['nan']} NaN, "
                          f"the image has {len(counted)} and {nan}")
        distribution = group["distribution"]
        values = sorted(value for value, _ in counted)
        low = min(counted, key=lambda sample: (sample[0], sample[1]))
        high = min(counted, key=lambda sample: (-sample[0], sample[1]))
        wanted = {
            "min": low,
            "max": high,
            "p50": rank(values, 0.5),
            "p90": rank(values, 0.9),
            "p99": rank(values, 0.99),
            "mean": sum(values) / len(values),
        }
        for name, value in wanted.items():
            if name in ("min", "max"):
                found = (distribution[name]["value"], distribution[name]["index"])
                fits = same(found[0], value[0]) and found[1] == value[1]
            else:
                found = distribution[name]
                fits = same(found, float(value) if name == "mean" else value)
            if not fits:
                errors.append(f"stats {curve} {name}: {found}, the image has {value}")
    return errors


def compare_text(stdout: str, curves: dict) -> list[str]:
    errors = []
    for curve in curves:
        subject, field = curve.rsplit(".", 1)
        if not any(line.startswith(f"value {subject} value {field}: ") for line in stdout.splitlines()):
            errors.append(f"kymo stats writes no line for {curve}:\n{stdout}")
    return errors


def compare_check(host: str, capture: str, curves: dict) -> list[str]:
    errors = []

    def check(rules: list[dict], code: int, label: str) -> None:
        with tempfile.TemporaryDirectory() as folder:
            path = os.path.join(folder, "rules.json")
            with open(path, "w", encoding="utf-8") as file:
                json.dump({"rules": rules}, file)
            done = run([host, "check", capture, path])
        if done.returncode != code:
            errors.append(f"check {label}: exit {done.returncode}, not {code}\n{done.stdout}{done.stderr}")

    def rule(curve: str, stat: str, **bounds) -> dict:
        subject, field = curve.rsplit(".", 1)
        return {"subject": subject, "metric": "value", "detail": field, "stat": stat, **bounds}

    def known(curve: str) -> list:
        return [value for value, _ in curves[curve] if not (isinstance(value, float) and math.isnan(value))]

    x_max = max(known("axis::setpoint.x"))
    z_min = min(known("axis::setpoint.z"))
    f_max = max(known("gauge.f"))
    check([rule("axis::setpoint.x", "max", max=x_max),
           rule("axis::setpoint.z", "min", min=z_min),
           rule("gauge.f", "max", max=f_max),
           rule("label.n", "count", min=len(known("label.n")), max=len(known("label.n")))], 0, "at the values")
    check([rule("axis::setpoint.x", "max", max=x_max - 1)], 3, "x max one below")
    check([rule("axis::setpoint.z", "min", min=z_min + 1)], 3, "z min one above")
    check([rule("gauge.f", "max", max=f_max - 0.125)], 3, "f max below")
    check([rule("axis::setpoint.on", "max", max=1)], 1, "a bool")
    check([rule("label.c", "max", max=1)], 1, "a char")
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
    curves = expected_of(target.stdout)
    if target.returncode != 0 or "RESULT: PASS" not in target.stdout or not curves:
        print(target.stdout)
        print("the image did not run to its end")
        return 1

    errors = []
    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "values.kymo")
        recording = record_to_exit(args.qemu, args.machine, args.image, args.host, capture)
        if recording.returncode != 0 or recording.exit_status != 0:
            print(recording.stdout)
            print(recording.stderr)
            print("the recording did not reach the image's end")
            return 1
        stats = run([args.host, "stats", capture, "--json"])
        if stats.returncode != 0:
            errors.append(f"kymo stats --json failed: {stats.stderr.strip()}")
        else:
            errors += compare_stats(json.loads(stats.stdout), curves)
        text = run([args.host, "stats", capture])
        if text.returncode != 0:
            errors.append(f"kymo stats failed: {text.stderr.strip()}")
        else:
            errors += compare_text(text.stdout, curves)
        errors += compare_check(args.host, capture, curves)

    if errors:
        print("\n".join(errors))
        return 1
    samples = sum(len(samples) for samples in curves.values())
    print(f"{len(curves)} curves, {samples} samples read as the image emitted them")
    return 0


if __name__ == "__main__":
    sys.exit(main())
