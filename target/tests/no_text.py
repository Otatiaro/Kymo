#!/usr/bin/env python3
"""No event's text on the target: each is in the DWARF only.

    no_text.py --host kymo --objcopy OBJCOPY --image I

Reads the texts of the image's event types with kymo events, then the bytes
the image loads on the target (objcopy -O binary, the sections a probe
writes). The exit status is 1 when one of those texts is among those bytes,
or when the image declares no text, which would leave nothing to look for.
"""

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path


def texts(host, image):
    """The texts kymo events reads from the image's DWARF."""
    completed = subprocess.run([host, "events", str(image)], capture_output=True, text=True)
    if completed.returncode != 0:
        sys.exit(f"kymo events refused {image}:\n{completed.stderr}")
    listing = completed.stdout
    return [line.strip().removeprefix("text ") for line in listing.splitlines() if line.startswith("  text ")]


def loaded(objcopy, image):
    """The bytes the image puts on the target."""
    with tempfile.TemporaryDirectory() as directory:
        binary = Path(directory) / "image.bin"
        subprocess.run([objcopy, "-O", "binary", str(image), str(binary)], check=True)
        return binary.read_bytes()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", required=True)
    parser.add_argument("--objcopy", required=True)
    parser.add_argument("--image", required=True, type=Path)
    args = parser.parse_args()

    found = texts(args.host, args.image)
    if not found:
        print(f"{args.image}: no event declares a text")
        return 1
    image = loaded(args.objcopy, args.image)
    on_target = [text for text in found if text.encode() in image]
    for text in on_target:
        print(f"on the target: {text!r}")
    print(f"{len(found)} texts, {len(on_target)} on the target")
    return 1 if on_target else 0


if __name__ == "__main__":
    sys.exit(main())
