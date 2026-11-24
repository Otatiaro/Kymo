#!/usr/bin/env python3
"""Runs clang-tidy over the target code, as the Arm GNU toolchain compiles it.

    tidy.py --clang-tidy CLANG_TIDY --build BUILD [--jobs N] [--headers DIR]... [FILE...]

BUILD is a build directory configured with CMAKE_EXPORT_COMPILE_COMMANDS=ON.
Each source file is checked once per set of preprocessor definitions it is
built with; the CPU and the optimisation level do not change what clang-tidy
sees. Clang does not find the headers of a GCC cross toolchain by itself, so
each compiler is asked for its include directories, which are passed to
clang-tidy instead of clang's own.

The checks and the files they cover are those of the .clang-tidy files. The
exit status is 1 when clang-tidy reports anything.
"""

import argparse
import concurrent.futures
import json
import shlex
import subprocess
import sys
from pathlib import Path

# Compiler options that change what clang-tidy sees, besides definitions.
KEPT = ("-I", "-isystem", "-D", "-U", "-std=", "-mcpu=", "-mthumb", "-mfloat-abi=", "-mfpu=")


def arguments(entry):
    """The compiler and its arguments for one compile_commands entry."""
    if "arguments" in entry:
        return entry["arguments"]
    return shlex.split(entry["command"], posix=False)


def kept(args):
    """The options of @p args that clang-tidy needs, in order."""
    out = []
    pending = False
    for arg in args[1:]:
        if pending:
            out.append(arg)
            pending = False
        elif arg in ("-I", "-isystem", "-D", "-U"):
            out.append(arg)
            pending = True
        elif arg.startswith(KEPT):
            out.append(arg)
    return out


def include_directories(compiler, options):
    """The system include directories @p compiler searches with @p options."""
    flags = [o for o in options if o.startswith(("-std=", "-mcpu=", "-mthumb", "-mfloat-abi=", "-mfpu="))]
    result = subprocess.run([compiler.strip('"'), *flags, "-xc++", "-E", "-v", "-"],
                            input="", capture_output=True, text=True, check=True)
    lines = result.stderr.splitlines()
    start = lines.index("#include <...> search starts here:") + 1
    end = lines.index("End of search list.")
    return [str(Path(line.strip()).resolve()) for line in lines[start:end]]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--clang-tidy", required=True)
    parser.add_argument("--build", required=True, type=Path)
    parser.add_argument("--jobs", type=int, default=None)
    parser.add_argument("--headers", type=Path, action="append", default=[],
                        help="a directory whose headers are each checked on their own")
    parser.add_argument("files", nargs="*", type=Path)
    options = parser.parse_args()

    entries = json.loads((options.build / "compile_commands.json").read_text())
    wanted = {f.resolve() for f in options.files}
    seen = set()
    units = []
    directories = {}
    for entry in entries:
        source = Path(entry["file"]).resolve()
        if wanted and source not in wanted:
            continue
        args = arguments(entry)
        flags = kept(args)
        definitions = tuple(sorted(a for a in flags if a.startswith(("-D", "-U"))))
        if (source, definitions) in seen:
            continue
        seen.add((source, definitions))
        key = (args[0], tuple(f for f in flags if f.startswith(("-std=", "-mcpu="))))
        if key not in directories:
            directories[key] = include_directories(args[0], flags)
        units.append((source, flags, directories[key]))

    # A header is checked with the configuration of the file it is checked
    # from, and misc-include-cleaner looks at that file alone: through a test,
    # a header would get the test's checks. Each header is checked as a file
    # of its own, with the options of the first C++ source of the build.
    if options.headers:
        first = arguments(next(e for e in entries if Path(e["file"]).suffix in (".cpp", ".cc", ".cxx")))
        flags = kept(first)
        key = (first[0], tuple(f for f in flags if f.startswith(("-std=", "-mcpu="))))
        if key not in directories:
            directories[key] = include_directories(first[0], flags)
        for directory in options.headers:
            units += [(header.resolve(), flags, directories[key]) for header in sorted(directory.rglob("*.hpp"))]

    if not units:
        sys.exit("no source to check")

    def check(unit):
        source, flags, includes = unit
        extra = ["--target=arm-none-eabi", "-nostdinc", *flags]
        for directory in includes:
            extra += ["-isystem", directory]
        command = [options.clang_tidy, "--quiet", str(source), "--"] + extra
        result = subprocess.run(command, capture_output=True, text=True)
        return source, result.returncode, result.stdout + result.stderr

    failed = 0
    with concurrent.futures.ThreadPoolExecutor(options.jobs) as pool:
        for source, code, output in pool.map(check, units):
            report = "\n".join(line for line in output.splitlines()
                               if "warnings generated" not in line and "warning generated" not in line)
            if code != 0 or "warning:" in report or "error:" in report:
                failed += 1
                print(report.strip())
    print(f"{len(units)} files checked, {failed} with findings")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
