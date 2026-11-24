#!/usr/bin/env python3
"""Record OpSy's own suite through QEMU's monitor, and check the trace.

    trace.py --qemu Q --machine M --image I --host KYMO

Runs OpSy's suite traced by the adapter (opsy_qemu_tests) under QEMU, paused
until `kymo record --probe qemu` has started, so that the recording holds the
suite from its first event; the guest runs at the pace of the host's clock,
so that the passes keep up with it. The guest halts on a breakpoint at the
suite's exit (QEMU would leave there), set through QEMU's GDB stub, so that
the recording holds the whole suite; then the recording stops. Then checks
that:

- every event is one of the adapter's, with its fields;
- every task, condition variable, mutex and handler is named from the ELF,
  never printed in hexadecimal;
- the task an event says is running is the one the scheduler last put on the
  core (task_started, or none after enter_idle): the task of task_sleep,
  task_wait and task_wait_timeout; and the task that takes a mutex or blocks
  on one, unless it is waiting, which the task waking it does for it: a
  waiter handed the mutex it was blocked on, or a task whose wait on a
  condition ends, taking back the mutex it waited with or blocking on it;
- every isr_exit closes the last isr_enter still open, of the same handler;
- every task_priority_changed moves the task's priority away from the last
  one task_added or task_priority_changed gave it, and the suite's mutex
  donations make the priority both rise and drop;
- every family on by default was recorded;
- `kymo decode --opsy` on the capture of the recording gives the spans
  computed here from its event lines: tasks on the core, handlers running,
  mutexes held and waited for.

After a loss, the running task, the waiting ones, the open interrupts and the
priorities are unknown until seen again: a check that needs them is skipped
meanwhile.
"""

import argparse
import os
import re
import socket
import struct
import subprocess
import sys
import tempfile
import threading

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "tests", "record"))
from record import TIMEOUT_SECONDS, free_port, wait_for  # noqa: E402

# Where the suite ends the emulation: qemu::exit(int), out of line at every
# optimisation level.
EXIT = "_ZN4qemu4exitEi"

PREFIX = "kymo::opsy::"
# Each event of the adapter: its family and its fields, in order.
EVENTS = {
    "starting": ("scheduling", ["core_clock"]),
    "task_added": ("scheduling", ["task", "priority"]),
    "task_terminated": ("scheduling", ["task"]),
    "task_started": ("scheduling", ["task"]),
    "enter_idle": ("scheduling", []),
    "task_priority_changed": ("scheduling", ["task", "priority"]),
    "task_ready": ("causality", ["task", "reason"]),
    "task_sleep": ("causality", ["task", "until"]),
    "task_wait": ("causality", ["task", "condition"]),
    "task_wait_timeout": ("causality", ["task", "condition", "until"]),
    "condition_variable_notify_one": ("causality", ["condition"]),
    "condition_variable_notify_all": ("causality", ["condition"]),
    "enter_systick": ("kernel", []),
    "exit_systick": ("kernel", ["task_switch"]),
    "enter_service_call": ("kernel", []),
    "exit_service_call": ("kernel", ["task_switch"]),
    "enter_pend_sv": ("kernel", []),
    "isr_enter": ("interrupts", ["handler"]),
    "isr_exit": ("interrupts", ["handler"]),
    "mutex_taken": ("exclusion", ["mutex", "owner"]),
    "mutex_released": ("exclusion", ["mutex", "owner"]),
    "mutex_blocked": ("exclusion", ["mutex", "waiter"]),
}
DEFAULT_FAMILIES = {"scheduling", "causality", "interrupts", "exclusion"}
POINTERS = {"task", "condition", "mutex", "owner", "waiter", "handler"}
# The field holding the task that must be the running one.
RUNNING = {"task_sleep": "task", "task_wait": "task", "task_wait_timeout": "task"}
# The same, unless that task is waiting and the one waking it acts for it.
RUNNING_OR_WAITING = {"mutex_taken": "owner", "mutex_blocked": "waiter"}
# Where a task starts waiting: until it is put on the core again.
WAITS = {"task_wait", "task_wait_timeout", "mutex_blocked"}
UNKNOWN = object()


def parse(output: str) -> list:
    """The events and losses, in order: (line, name, fields) or (line, None, None).

    A recording that starts after the target's first event starts with a loss:
    what happened before it is unknown.
    """
    items = []
    for line in output.splitlines():
        words = line.split(" ", 5)
        if words[0] == "record" and int(words[1]) != 0:
            items.append((line, None, None))
        elif words[0] == "lost":
            items.append((line, None, None))
        elif words[0] == "event":
            # A name may hold a space, "(anonymous namespace)": a field
            # starts where a word is followed by "=".
            fields = re.split(r" (?=\w+=)", words[5]) if len(words) > 5 else []
            items.append((line, words[4], [tuple(field.split("=", 1)) for field in fields]))
    return items


def check(output: str) -> list[str]:
    errors = []
    items = parse(output)
    families = set()
    running = UNKNOWN  # until the scheduler puts a task on the core
    waiting = set()
    known = None  # after a loss: the tasks put on the core since, whether they wait is known
    open_isrs = []  # None at the bottom when interrupts may have opened during a loss
    priorities = {}  # task -> its priority, lower is higher; only those seen since the last loss
    counts = {"running": 0, "waiting": 0, "isr_exit": 0, "priority_raised": 0, "priority_dropped": 0}
    for line, name, fields in items:
        if name is None:
            running, known, open_isrs = UNKNOWN, set(), [None]
            priorities.clear()
            continue
        if not name.startswith(PREFIX) or name[len(PREFIX):] not in EVENTS:
            errors.append(f"{line}: not an event of the adapter")
            continue
        name = name[len(PREFIX):]
        family, expected = EVENTS[name]
        families.add(family)
        if [field for field, _ in fields] != expected:
            errors.append(f"{line}: expected the fields {expected}")
            continue
        values = dict(fields)
        for field in POINTERS & values.keys():
            if values[field].startswith("0x"):
                errors.append(f"{line}: {field} is not named")
        if name == "task_started":
            running = values["task"]
            waiting.discard(running)
            if known is not None:
                known.add(running)
        elif name == "enter_idle":
            running = None
        elif name in RUNNING and running is not UNKNOWN:
            counts["running"] += 1
            if values[RUNNING[name]] != running:
                errors.append(f"{line}: {values[RUNNING[name]]} is not running, {running} is")
        elif name in RUNNING_OR_WAITING and running is not UNKNOWN:
            task = values[RUNNING_OR_WAITING[name]]
            if task == running:
                counts["running"] += 1
            elif task in waiting:
                counts["waiting"] += 1
            elif known is None or task in known:
                errors.append(f"{line}: {task} is neither running nor waiting, {running} is running")
        if name in WAITS:
            waiting.add(values["waiter" if name == "mutex_blocked" else "task"])
        elif name in ("task_terminated", "task_added"):
            waiting.discard(values["task"])
            if known is not None:
                known.add(values["task"])
        if name == "task_added":
            priorities[values["task"]] = int(values["priority"], 0)
        elif name == "task_priority_changed":
            priority = int(values["priority"], 0)
            previous = priorities.get(values["task"])
            if previous == priority:
                errors.append(f"{line}: the priority of {values['task']} did not move")
            elif previous is not None:
                counts["priority_raised" if priority < previous else "priority_dropped"] += 1
            priorities[values["task"]] = priority
        if name == "isr_enter":
            open_isrs.append(values["handler"])
        elif name == "isr_exit":
            if open_isrs == [None]:
                continue  # entered during a loss
            counts["isr_exit"] += 1
            if not open_isrs or open_isrs[-1] != values["handler"]:
                errors.append(f"{line}: the open interrupt is {open_isrs[-1] if open_isrs else 'none'}")
            else:
                open_isrs.pop()
    losses = sum(name is None for _, name, _ in items)
    print(f"{len(items) - losses} events, {losses} losses, families {sorted(families)}, "
          f"{counts['running']} running and {counts['waiting']} waiting tasks checked, "
          f"{counts['isr_exit']} interrupt exits checked, priority raised "
          f"{counts['priority_raised']} and dropped {counts['priority_dropped']} times")
    for family in sorted(DEFAULT_FAMILIES - families):
        errors.append(f"never recorded: family {family}")
    if 0 in counts.values():
        errors.append(f"the recording left a check unexercised: {counts}")
    return errors

def spans(output: str) -> list[str]:
    """OpSy's spans in the event lines of a recording, as kymo decode --opsy
    prints them, sorted.

    A task runs from its task_started, or the idle task from enter_idle, to
    the next of either. A handler runs from its isr_enter to the isr_exit
    that closes it, at the depth of the handlers still open below it. A mutex
    is held from mutex_taken to mutex_released by the same task, and waited
    for from mutex_blocked to its waiter's mutex_taken, or the waiter's
    task_terminated. A loss ends whatever is open; an exit or a release seen
    after one, with nothing open to close, starts at it. From a loss to the
    next task_started or enter_idle with no handler open, the depth is
    unknown. An event against that order is a contradiction.
    """
    out = []
    lost = False
    nesting_known = True
    running = None  # (task, since), or None while unknown
    isrs = []  # (handler, since, depth)
    held = {}  # mutex: (owner, since)
    waits = {}  # (mutex, waiter): since
    for line, name, fields in parse(output):
        if name is None:
            if running is not None:
                out.append(f"run from={running[1]} to=lost task={running[0]}")
            for handler, since, depth in isrs:
                out.append(f"interrupt from={since} to=lost depth={depth} handler={handler}")
            for mutex, (owner, since) in held.items():
                out.append(f"hold from={since} to=lost mutex={mutex} task={owner}")
            for (mutex, waiter), since in waits.items():
                out.append(f"wait from={since} to=lost mutex={mutex} task={waiter}")
            lost, nesting_known, running, isrs, held, waits = True, False, None, [], {}, {}
            continue
        at = line.split(" ")[1]
        name = name[len(PREFIX):] if name.startswith(PREFIX) else name
        values = dict(fields)
        if name in ("task_started", "enter_idle"):
            task = values.get("task", "idle")
            if running is not None:
                out.append(f"run from={running[1]} to={at} task={running[0]}")
            running = (task, at)
            if not isrs:
                nesting_known = True
        elif name == "isr_enter":
            isrs.append((values["handler"], at, len(isrs) if nesting_known else "unknown"))
        elif name == "isr_exit":
            handler = values["handler"]
            if isrs and isrs[-1][0] == handler:
                _, since, depth = isrs.pop()
            elif not isrs and lost:
                since, depth = "lost", "unknown"
            else:
                out.append(f"contradiction {at}")
                continue
            out.append(f"interrupt from={since} to={at} depth={depth} handler={handler}")
        elif name == "mutex_taken":
            mutex, owner = values["mutex"], values["owner"]
            if mutex in held:
                out.append(f"contradiction {at}")
                continue
            if (mutex, owner) in waits:
                out.append(f"wait from={waits.pop((mutex, owner))} to={at} mutex={mutex} task={owner}")
            held[mutex] = (owner, at)
        elif name == "mutex_released":
            mutex, owner = values["mutex"], values["owner"]
            if mutex in held and held[mutex][0] == owner:
                since = held.pop(mutex)[1]
            elif mutex not in held and lost:
                since = "lost"
            else:
                out.append(f"contradiction {at}")
                continue
            out.append(f"hold from={since} to={at} mutex={mutex} task={owner}")
        elif name == "mutex_blocked":
            key = (values["mutex"], values["waiter"])
            if key in waits:
                out.append(f"contradiction {at}")
                continue
            waits[key] = at
        elif name == "task_terminated":
            for mutex, waiter in [key for key in waits if key[1] == values["task"]]:
                out.append(f"wait from={waits.pop((mutex, waiter))} to={at} mutex={mutex} task={waiter}")
    if running is not None:
        out.append(f"run from={running[1]} to=open task={running[0]}")
    for handler, since, depth in isrs:
        out.append(f"interrupt from={since} to=open depth={depth} handler={handler}")
    for mutex, (owner, since) in held.items():
        out.append(f"hold from={since} to=open mutex={mutex} task={owner}")
    for (mutex, waiter), since in waits.items():
        out.append(f"wait from={since} to=open mutex={mutex} task={waiter}")
    return sorted(out)


def check_spans(output: str, decoded: str) -> list[str]:
    """kymo decode --opsy's spans against those computed here, each kind seen."""
    expected, got = spans(output), sorted(decoded.splitlines())
    kinds = {kind: sum(line.startswith(kind + " ") for line in expected)
             for kind in ("run", "interrupt", "hold", "wait")}
    print(f"spans: {kinds}, {len(got)} printed by kymo decode --opsy")
    errors = [f"expected, not printed: {line}" for line in sorted(set(expected) - set(got))]
    errors += [f"printed, not expected: {line}" for line in sorted(set(got) - set(expected))]
    if not errors and expected != got:
        errors.append("the same spans, not as many times each")
    if 0 in kinds.values():
        errors.append(f"the recording left a kind of span unexercised: {kinds}")
    return errors


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
    """Just enough of GDB's remote protocol to break on an address and run to it."""

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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    args = parser.parse_args()

    exit_address = symbol(args.image, EXIT)
    directory = tempfile.TemporaryDirectory()
    capture = os.path.join(directory.name, "suite.kcap")
    record_port, gdb_port = free_port(), free_port()
    # -S: paused until kymo record runs. align=on: the guest's clock does not
    # run ahead of the host's, so the passes keep up with it.
    target = subprocess.Popen(
        [args.qemu, "-machine", args.machine, "-kernel", args.image, "-nographic", "-S",
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

    stop = None
    try:
        wait_for(record_port)
        wait_for(gdb_port)
        gdb = Gdb(gdb_port)
        record = subprocess.Popen(
            [args.host, "record", "--probe", "qemu", "--probe-id", f"127.0.0.1:{record_port}",
             "--elf", args.image, "--out", capture],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        reader = threading.Thread(target=read, args=(record.stdout,))
        reader.start()
        if started.wait(TIMEOUT_SECONDS) and record.poll() is None:
            try:
                # The guest halts at the suite's exit, where QEMU would leave:
                # kymo record's last pass reads it all, then it is stopped.
                stop = gdb.run_to(exit_address, TIMEOUT_SECONDS)
            except (OSError, TimeoutError) as error:
                print(f"the suite did not reach {EXIT}: {error}")
        record.stdin.write("\n")
        record.stdin.close()
        record.wait(TIMEOUT_SECONDS)
        reader.join(TIMEOUT_SECONDS)
        stderr = record.stderr.read()
    finally:
        target.kill()
        target.wait()
    stdout = "".join(lines)
    print(stderr.strip())
    if record.returncode != 0 or stop is None:
        print(stdout[-2000:])
        print("kymo record failed" if record.returncode != 0 else f"stopped short of {EXIT}")
        return 1
    errors = check(stdout)
    decoded = subprocess.run([args.host, "decode", args.image, capture, "--opsy"],
                             capture_output=True, text=True, timeout=TIMEOUT_SECONDS)
    if decoded.returncode != 0:
        errors.append(f"kymo decode --opsy failed: {decoded.stderr.strip()}")
    else:
        errors += check_spans(stdout, decoded.stdout)
    directory.cleanup()
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
