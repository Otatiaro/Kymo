#!/usr/bin/env python3
"""Compare a table produced by reference.cpp with the committed one.

    compare.py COMMITTED PRODUCED KNOWN

Each line of a table is `<type>\t<spec>\t<value>\t<output>`. The cases on
which the two tables differ must be exactly those KNOWN lists, one
`<type>\t<spec>\t<value>` per line (`#` starts a comment): a new difference
fails, and so does a known one that is gone, so that its entry is removed.
"""

import sys


def cases(path: str) -> list:
    """The table's cases in order, each as (type, spec and value; output)."""
    table = []
    with open(path, "rb") as file:
        for line in file.read().split(b"\n"):
            if line:
                ty, spec, value, output = line.split(b"\t", 3)
                table.append((b"\t".join((ty, spec, value)), output))
    return table


def main() -> int:
    committed, produced = cases(sys.argv[1]), cases(sys.argv[2])
    with open(sys.argv[3], "rb") as file:
        known = {line for line in file.read().split(b"\n") if line and not line.startswith(b"#")}

    if [key for key, _ in committed] != [key for key, _ in produced]:
        print("the two tables do not hold the same cases")
        return 1
    differ = set()
    for (key, written), (_, other) in zip(committed, produced):
        if written != other:
            differ.add(key)
            if key not in known:
                print(f"differs: {key!r}: committed {written!r}, produced {other!r}")
    for key in sorted(known - differ):
        print(f"known to differ, but agrees now: {key!r}")
    if differ != known:
        return 1
    print(f"{len(committed)} cases, {len(known)} known to differ")
    return 0


if __name__ == "__main__":
    sys.exit(main())
