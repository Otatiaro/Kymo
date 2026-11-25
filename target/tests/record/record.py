#!/usr/bin/env python3
"""Record a running image through QEMU's monitor, and check what was recorded.

    record.py --qemu Q --machine M --image I --host KYMO
              [--lap follows|behind --icount-shift N | --killed running|paused]

Runs the image under QEMU with its monitor on a TCP port, records it with
`kymo record --probe qemu` for a few seconds into a capture file, then
checks the output (record.cpp). QEMU starts stopped, and runs once the host
has read the ring a first time: the host sees every event from the first,
however long it takes to start.

The output must show that:

- every index from where recording started to where it stopped is in one
  event or one loss, in order, never both, never twice, and a loss is never
  followed by another;
- every event holds the values its image wrote for its index;
- the recording went through losses, every type, and events whose slots run
  past the end of the ring;
- no writer was reported suspect without --lap: none is held for a lap;
- `kymo decode` of the capture prints exactly what `kymo record` printed:
  they may differ only by the seconds, which are all 0 here (QEMU's
  timestamps all read 0) at the frequency given and kept in the capture.

With --lap, the image holds writers for more than a lap (lap.cpp), and the
`held` events show the ring lapped some while recording. An event may hold
another's bytes only near a suspect line, from a ring before its first index
to its last. With `follows`, the host reads held writers before the lap, but
a loaded machine may still delay a read past it: a suspect line is then
correct, and tolerated. With `behind`, the lap falls between two reads: some
writers must be suspect.

With --killed, the host is killed while recording, without --duration:
the capture has no end record. `kymo decode` must read it up to its last
whole record, say where it was cut, and print what `kymo record` printed up
to there, every index accounted for up to the index it reports. `running`
kills the host amid the traffic, where the capture may end inside a record.
`paused` stops QEMU first, and kills the host once it has made a few passes
over the still ring: the host puts each pass on disk, so the capture must
hold all it printed, and end on a whole record.

Which indexes are lost depends on how QEMU and the host interleave, so the
check does not predict them: it checks that the output accounts for each.

The adapters' trace checks import record_to_exit from here: an image
recorded under QEMU from its first event to its exit.
"""

import argparse
import json
import os
import re
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

RECORD_SECONDS = "2"
LAP_RECORD_SECONDS = "4"
PAUSED_SECONDS = 0.1  # the host makes a few passes over the stopped target
CLOCK_HZ = "64000000"  # given, so that the capture carries it to kymo decode
TIMEOUT_SECONDS = 60
SLOTS = 16  # ring_slots of the test images (test_trace.hpp)
TYPES = {"one": 1, "two": 2, "four": 4}  # slots each type takes
LAP_TYPES = {**TYPES, "held": 1}


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def wait_for(port: int) -> None:
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while True:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
            return
        except OSError:
            if time.monotonic() > deadline:
                raise
            time.sleep(0.05)


def resume(port: int, command: str = "cont") -> None:
    """Runs the stopped QEMU, or with "stop" stops it, through its second monitor."""
    with socket.create_connection(("127.0.0.1", port), timeout=TIMEOUT_SECONDS) as monitor:
        replies = monitor.makefile("r")
        replies.readline()  # the greeting
        for command in ("qmp_capabilities", command):
            monitor.sendall(json.dumps({"execute": command}).encode() + b"\n")
            reply = json.loads(replies.readline())
            while "return" not in reply and "error" not in reply:
                reply = json.loads(replies.readline())  # an event
            if "error" in reply:
                raise RuntimeError(f"QEMU refused {command}: {reply['error']}")


def symbol(image: str, name: str) -> int:
    """The address of a function of an ELF32 little-endian image, from its symbol table."""
    with open(image, "rb") as file:
        elf = file.read()
    shoff, = struct.unpack_from("<I", elf, 0x20)
    shentsize, shnum = struct.unpack_from("<HH", elf, 0x2E)
    sections = [struct.unpack_from("<IIIIIIIIII", elf, shoff + i * shentsize) for i in range(shnum)]
    for _, kind, _, _, offset, size, link, _, _, entsize in sections:
        if kind != 2:  # SHT_SYMTAB
            continue
        strings = sections[link][4]
        for at in range(offset, offset + size, entsize):
            name_at, value = struct.unpack_from("<II", elf, at)
            end = elf.index(b"\0", strings + name_at)
            if elf[strings + name_at:end].decode() == name:
                return value & ~1
    raise LookupError(f"{image} has no symbol {name}")


class Gdb:
    """Just enough of GDB's remote protocol to break on an address, run to it
    and read a register."""

    def __init__(self, port: int):
        self.stream = socket.create_connection(("127.0.0.1", port), timeout=TIMEOUT_SECONDS)

    def send(self, packet: str) -> str:
        self.stream.sendall(f"${packet}#{sum(packet.encode()) % 256:02x}".encode())
        reply = b""
        while not re.fullmatch(rb"\+*\$[^#]*#[0-9a-f]{2}", reply):
            chunk = self.stream.recv(4096)
            if not chunk:
                raise OSError("QEMU closed the GDB stub")
            reply += chunk
        self.stream.sendall(b"+")
        return reply.decode().lstrip("+")[1:-3]

    def run_to(self, address: int, timeout: float) -> str:
        if self.send(f"Z0,{address:x},2") != "OK":
            raise OSError("the GDB stub set no breakpoint")
        self.stream.settimeout(timeout)
        return self.send("c")

    def register(self, number: int) -> int:
        """A core register, r0 to r15, of the stopped target."""
        self.stream.settimeout(TIMEOUT_SECONDS)
        return int.from_bytes(bytes.fromhex(self.send(f"p{number:x}")), "little")


# Where a test image ends the emulation: qemu::exit(int), out of line at
# every optimisation level.
EXIT = "_ZN4qemu4exitEi"


class Recording:
    """What record_to_exit gives: kymo record's output and status, and the
    status the image ends with, None if it did not reach its end."""

    def __init__(self, stdout: str, stderr: str, returncode: int, exit_status: int | None):
        self.stdout, self.stderr, self.returncode, self.exit_status = stdout, stderr, returncode, exit_status


def record_to_exit(qemu: str, machine: str, image: str, host: str, capture: str, end: str = EXIT) -> Recording:
    """Records an image through QEMU's monitor, from its first event to its exit.

    QEMU starts paused until kymo record has started, so that the recording
    holds the image from its first event; the guest runs at the pace of the
    host's clock, so that the passes keep up with it. The guest halts on a
    breakpoint at the function `end`, qemu::exit by default (QEMU would leave
    there), set through QEMU's GDB stub: kymo record reads it all, then it is
    stopped. The status is r0 there, end's first argument.
    """
    end_address = symbol(image, end)
    record_port, gdb_port = free_port(), free_port()
    # -S: paused until kymo record runs. align=on: the guest's clock does not
    # run ahead of the host's.
    target = subprocess.Popen(
        [qemu, "-machine", machine, "-kernel", image, "-nographic", "-S",
         "-icount", "shift=5,align=on,sleep=on",
         "-semihosting-config", "enable=on,target=native", "-monitor", "none", "-serial", "none",
         "-qmp", f"tcp:127.0.0.1:{record_port},server=on,wait=off",
         "-gdb", f"tcp:127.0.0.1:{gdb_port}"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    lines = []
    started = threading.Event()

    def read(stream) -> None:
        for line in stream:
            lines.append(line)
            if line.startswith("record "):
                started.set()
        started.set()

    exit_status = None
    try:
        wait_for(record_port)
        wait_for(gdb_port)
        gdb = Gdb(gdb_port)
        record = subprocess.Popen(
            [host, "record", "--probe", "qemu", "--probe-id", f"127.0.0.1:{record_port}",
             "--elf", image, "--out", capture],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8")
        reader = threading.Thread(target=read, args=(record.stdout,))
        reader.start()
        if started.wait(TIMEOUT_SECONDS) and record.poll() is None:
            try:
                gdb.run_to(end_address, TIMEOUT_SECONDS)
                exit_status = gdb.register(0)
                # kymo record stops on what it last read: passes over the
                # halted guest until one prints nothing new.
                printed = -1
                while printed != len(lines):
                    printed = len(lines)
                    time.sleep(PAUSED_SECONDS)
            except (OSError, TimeoutError) as error:
                print(f"the image did not reach {end}: {error}")
        try:
            record.stdin.write("\n")
            record.stdin.close()
        except OSError:
            pass  # kymo record has stopped already: QEMU left
        record.wait(TIMEOUT_SECONDS)
        reader.join(TIMEOUT_SECONDS)
        stderr = record.stderr.read()
    finally:
        target.kill()
        target.wait()
    return Recording("".join(lines), stderr, record.returncode, exit_status)


def expected_fields(name: str, index: int) -> dict[str, str]:
    if name == "one":
        return {"index": str(index)}
    if name == "two":
        return {"index": str(index), "words": f"[{index + 1},{index + 2},{index + 3}]"}
    words = ",".join(str((index * 10 + k) & 0xFFFFFFFF) for k in range(10))
    return {"index": str(index), "words": f"[{words}]"}


def holds_its_values(name: str, index: int, values: dict[str, str]) -> bool:
    if name == "held":
        return values.keys() == {"index", "lapped"} and values["index"] == str(index)
    return values == expected_fields(name, index)


def check(output: str, lap: str | None) -> list[str]:
    errors = []
    types = LAP_TYPES if lap else TYPES
    lines = output.splitlines()
    if len(lines) < 2 or not lines[0].startswith("record ") or not lines[-1].startswith("end "):
        return ["the output does not run from a record line to an end line"]
    expected = int(lines[0].split()[1])
    events = dict.fromkeys(types, 0)
    losses = across = 0
    previous = None
    suspects = []  # (from, to) of each suspect line
    wrong = []  # (index, line) of each event that does not hold its values
    lapped = []
    for line in lines[1:-1]:
        fields = line.split()
        if fields[0] == "lost":
            start, end = int(fields[1]), int(fields[2])
            if start != expected or end <= start:
                errors.append(f"{line}: expected a loss from {expected}")
            if previous == "lost":
                errors.append(f"{line}: a loss right after a loss")
            previous = "lost"
            losses += 1
            expected = end
            continue
        if fields[0] == "suspect":
            suspects.append((int(fields[1]), int(fields[2])))
            continue
        previous = "event"
        # One index, or the range of the event's slots: "12-13".
        first, _, last = fields[1].partition("-")
        index, name = int(first), fields[4]
        if fields[0] != "event" or name not in types:
            errors.append(f"{line}: not an event of the image")
            continue
        if (int(last) if last else index) - index + 1 != types[name]:
            errors.append(f"{line}: expected {types[name]} slots")
        # QEMU has no cycle counter: no cycles, so 0 s at the frequency given.
        if fields[2:4] != ["0", "0.000000000"]:
            errors.append(f"{line}: expected 0 cycles at 0 s")
        values = dict(field.split("=", 1) for field in fields[5:])
        if index != expected:
            errors.append(f"{line}: expected index {expected}")
        if not holds_its_values(name, index, values):
            wrong.append((index, line))
        elif name == "held":
            lapped.append(int(values["lapped"]))
        events[name] += 1
        across += index % SLOTS + types[name] > SLOTS
        expected = index + types[name]
    end = int(lines[-1].split()[1])
    if expected != end:
        errors.append(f"the events and losses reach {expected}, the recording ends at {end}")
    print(f"events {events}, {losses} losses, {across} across the end of the ring, {len(suspects)} suspect lines, "
          f"{len(wrong)} events not holding their values, lapped from {min(lapped, default=0)} "
          f"to {max(lapped, default=0)}")
    if losses == 0 or across == 0 or 0 in events.values():
        errors.append("the recording did not go through losses, every type and the end of the ring")
    if lap and max(lapped, default=0) == min(lapped, default=0):
        errors.append("no writer was lapped while recording")
    if lap == "behind" and not suspects:
        errors.append("no writer was reported suspect")
    if lap:
        wrong = [(index, line) for index, line in wrong
                 if not any(start - SLOTS <= index < stop for start, stop in suspects)]
    elif suspects:
        errors.append(f"{len(suspects)} suspect lines, the first: from {suspects[0][0]} to {suspects[0][1]}")
    errors += [f"{line}: does not hold the values written for its index" for _, line in wrong]
    return errors


def killed(args, port: int, control: int, capture: str) -> int:
    """Kills the host while it records, then reads the capture it left."""
    # stdin open and silent: the host stops only when killed.
    record = subprocess.Popen(
        [args.host, "record", "--probe", "qemu", "--probe-id", f"127.0.0.1:{port}",
         "--elf", args.image, "--out", capture, "--clock", CLOCK_HZ],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    lines = []
    reader = threading.Thread(target=lambda: lines.extend(record.stdout))
    first = record.stdout.readline()
    if not first.startswith("record "):
        record.kill()
        print(f"kymo record printed {first!r} first")
        return 1
    lines.append(first)
    reader.start()
    resume(control)
    time.sleep(float(RECORD_SECONDS))
    if args.killed == "paused":
        resume(control, "stop")
        time.sleep(PAUSED_SECONDS)
    record.kill()
    record.wait()
    reader.join()
    record.stdin.close()

    errors = []
    replay = subprocess.run([args.host, "decode", capture],
                            capture_output=True, text=True, timeout=TIMEOUT_SECONDS)
    cut = re.search(r"cut short at byte (\d+), after index (\d+)", replay.stderr)
    if replay.returncode != 0:
        errors.append(f"kymo decode of the capture failed: {replay.stderr.strip()}")
    elif not cut:
        errors.append(f"kymo decode does not say where the capture was cut: {replay.stderr.strip()}")
    else:
        decoded = replay.stdout.splitlines(keepends=True)
        print(f"kymo record printed {len(lines)} lines, killed; the capture holds {len(decoded)}, "
              f"cut at byte {cut[1]} of {os.path.getsize(capture)}, after index {cut[2]}")
        if decoded != lines[:len(decoded)]:
            errors.append("kymo decode of the capture is not what kymo record printed up to the cut")
        elif args.killed == "paused" and len(decoded) != len(lines):
            errors.append(f"the capture lacks the last {len(lines) - len(decoded)} lines kymo record printed")
        if args.killed == "paused" and int(cut[1]) != os.path.getsize(capture):
            errors.append("the capture of a stopped target ends inside a record")
        errors += check(replay.stdout + f"end {cut[2]}\n", None)
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    parser.add_argument("--lap", choices=["follows", "behind"])
    parser.add_argument("--icount-shift", type=int)
    parser.add_argument("--killed", choices=["running", "paused"], help="kill the host while it records")
    args = parser.parse_args()

    port, control = free_port(), free_port()
    target = subprocess.Popen(
        [args.qemu, "-machine", args.machine, "-kernel", args.image, "-nographic", "-S",
         "-semihosting-config", "enable=on,target=native", "-monitor", "none", "-serial", "none",
         "-qmp", f"tcp:127.0.0.1:{port},server=on,wait=off",
         "-qmp", f"tcp:127.0.0.1:{control},server=on,wait=off"]
        + (["-icount", f"shift={args.icount_shift},sleep=off"] if args.icount_shift is not None else []),
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "record.kcap")
        try:
            wait_for(port)
            if args.killed:
                return killed(args, port, control, capture)
            record = subprocess.Popen(
                [args.host, "record", "--probe", "qemu", "--probe-id", f"127.0.0.1:{port}",
                 "--elf", args.image, "--duration", LAP_RECORD_SECONDS if args.lap else RECORD_SECONDS,
                 "--out", capture, "--clock", CLOCK_HZ],
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            first = record.stdout.readline()  # printed once the host has read the ring
            if first.startswith("record "):
                resume(control)
            rest, stderr = record.communicate(timeout=TIMEOUT_SECONDS)
            output = first + rest
        finally:
            target.kill()
            target.wait()
        print(stderr.strip())
        if record.returncode != 0:
            print(output[-2000:])
            print("kymo record failed")
            return 1
        errors = check(output, args.lap)
        replay = subprocess.run([args.host, "decode", capture],
                                capture_output=True, text=True, timeout=TIMEOUT_SECONDS)
        if replay.returncode != 0:
            errors.append(f"kymo decode of the capture failed: {replay.stderr.strip()}")
        elif replay.stdout != output:
            errors.append("kymo decode of the capture differs from what kymo record printed")
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
