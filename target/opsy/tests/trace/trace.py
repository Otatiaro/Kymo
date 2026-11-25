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

- every event is one of the adapter's, or of the application's durations
  (durations_check.cpp), with its fields;
- every task, condition variable, mutex and handler is named from the ELF,
  never printed in hexadecimal;
- the task an event says is running is the one the scheduler last put on the
  core (task_started, or none after enter_idle): the task of task_sleep,
  task_wait and task_wait_timeout; and the task that takes a mutex or blocks
  on one, unless it is waiting, which the task waking it does for it: a
  waiter handed the mutex it was blocked on, or a task whose wait on a
  condition ends, taking back the mutex it waited with or blocking on it;
- every isr_enter carries a device interrupt's exception number, and every
  isr_exit closes the last isr_enter still open, of the same handler;
- every task_priority_changed moves the task's priority away from the last
  one task_added or task_priority_changed gave it, and the suite's mutex
  donations make the priority both rise and drop;
- every family on by default was recorded;
- `kymo decode --meaning` on the capture of the recording gives the spans
  computed here from its event lines: tasks on the core, handlers running,
  mutexes held and waited for, the cause of each wake and each time a task
  is ready; and the suite wakes a task from a handler, from a task, by a
  delay and by a mutex released;
- `kymo stats --json`, on the whole capture and on its middle half, gives
  what oracle.py computes from the event lines `kymo decode` prints: every
  group, its counts, percentiles, histogram and worst instances, the
  durations included; and the handover a task begins and a handler ends
  after the task's end is one duration, with no contradiction;
- `kymo check` passes at the oracle's bounds of every group, durations
  included, and fails one cycle past them.

After a loss, the running task, the waiting ones, the open interrupts and the
priorities are unknown until seen again: a check that needs them is skipped
meanwhile.
"""

import argparse
import bisect
import json
import math
import os
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "tests", "record"))
from record import EXIT, TIMEOUT_SECONDS, record_to_exit  # noqa: E402

import oracle  # noqa: E402

# The worst instances kymo stats keeps for each group, in the comparison.
WORST = 5

PREFIX = "kymo::opsy::"
# The wake reasons of a delay that ran out, given in SysTick.
ELAPSED = {"timed_out", "sleep_elapsed"}
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
    "isr_enter": ("interrupts", ["handler", "exception"]),
    "isr_exit": ("interrupts", ["handler"]),
    "mutex_taken": ("exclusion", ["mutex", "owner"]),
    "mutex_released": ("exclusion", ["mutex", "owner"]),
    "mutex_blocked": ("exclusion", ["mutex", "waiter"]),
}
# The application's events, of durations_check.cpp and of the board's
# program (board/board.cpp): their fields, in order.
SAMPLING = "app::(anonymous namespace)::sampling"
APPLICATION = {
    "kymo::begin_of<handover>": ["item"],
    "kymo::end_of<handover>": [],
    "kymo::begin_of<job>": ["number"],
    "kymo::end_of<job>": [],
    f"kymo::begin_of<{SAMPLING}>": ["tick"],
    f"kymo::end_of<{SAMPLING}>": [],
    "app::(anonymous namespace)::setpoint": ["position"],
}
# The suite's durations: how many of each, none cut.
DURATIONS = {"handover": 1, "job": 2}
DEFAULT_FAMILIES = {"scheduling", "causality", "interrupts", "exclusion", "application"}
POINTERS = {"task", "condition", "mutex", "owner", "waiter", "handler"}
# The field holding the task that must be the running one.
RUNNING = {"task_sleep": "task", "task_wait": "task", "task_wait_timeout": "task"}
# The same, unless that task is waiting and the one waking it acts for it.
RUNNING_OR_WAITING = {"mutex_taken": "owner", "mutex_blocked": "waiter"}
# Where a task starts waiting: until it is put on the core again.
WAITS = {"task_wait", "task_wait_timeout", "mutex_blocked"}
# The exception number, IPSR's, of the first device interrupt.
FIRST_IRQ = 16
UNKNOWN = object()


def parse(output: str) -> list:
    """The events and losses, in order: (line, name, fields) or (line, None, None).

    A recording that starts after the target's first event starts with a loss:
    what happened before it is unknown.
    """
    items = []
    for line in output.splitlines():
        words = line.split(" ", 4)
        if words[0] == "record" and int(words[1]) != 0:
            items.append((line, None, None))
        elif words[0] == "lost":
            items.append((line, None, None))
        elif words[0] == "event":
            # A name or a value may hold a space, "(anonymous namespace)": a
            # field starts where a word is followed by "=".
            name, *fields = re.split(r" (?=\w+=)", words[4])
            items.append((line, name, [tuple(field.split("=", 1)) for field in fields]))
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
        if name in APPLICATION:
            families.add("application")
            if [field for field, _ in fields] != APPLICATION[name]:
                errors.append(f"{line}: expected the fields {APPLICATION[name]}")
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
            if int(values["exception"], 0) < FIRST_IRQ:
                errors.append(f"{line}: exception {values['exception']} is not a device interrupt")
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

def spans(output: str, names: dict[int, str]) -> list[str]:
    """OpSy's spans in the event lines of a recording, as kymo decode --meaning
    prints them, sorted.

    A task runs from its task_started, or the idle task from enter_idle, to
    the next of either. A handler runs from its isr_enter to the isr_exit
    that closes it, at the depth of the handlers still open below it; the
    kernel's SysTick and SVCall handlers likewise, from their enter_ event to
    their exit_ event. A handler's subject names its interrupt from `names`
    (oracle.handler_subject); an exit closes the innermost handler entered,
    of its address. A mutex
    is held from mutex_taken to mutex_released by the same task, and waited
    for from mutex_blocked to its waiter's mutex_taken, or the waiter's
    task_terminated. A loss ends whatever is open; an exit or a release seen
    after one, with nothing open to close, starts at it. From a loss to the
    next task_started or enter_idle with no handler open, the depth is
    unknown. An event against that order is a contradiction.

    Each task_ready has a cause: `elapsed` for a delay that ran out, or else
    the innermost handler open but the kernel's, or else the task running;
    `unknown` from a loss to the next task_started or enter_idle. A task is
    ready, in an activation (from its task_ready to its task_sleep, task_wait,
    task_wait_timeout, mutex_blocked or task_terminated), while it does not
    run. A loss ends the activations open; one that ends after it with no
    task_ready seen since, and none open at it, starts at it.
    """
    out = []
    lost = None  # the last loss, as an edge (position, label)
    nesting_known = True
    running = None  # (task, since), or None while unknown
    isrs = []  # (subject, since, depth, handler)
    held = {}  # mutex: (owner, since)
    waits = {}  # (mutex, waiter): since
    ready = {}  # task: since, of its activation open
    seen = set()  # tasks whose activation is known since the last loss
    activations = []  # (task, since, to, open), edges (position, label)
    runs = []  # (task, since, to), edges (position, label)
    last = None

    def end_activation(task, edge):
        if task in ready:
            activations.append((task, ready.pop(task), edge, False))
        elif lost is not None and task not in seen:
            activations.append((task, lost, edge, False))
        seen.add(task)

    def cause(reason):
        if reason in ELAPSED:
            return "elapsed"
        for subject, _, _, handler in reversed(isrs):
            if handler not in oracle.KERNEL.values():
                return f"handler:{subject}"
        return "unknown" if running is None else f"task:{running[0]}"

    for n, (line, name, fields) in enumerate(parse(output)):
        if name is None:
            edge = (n, "lost")
            if running is not None:
                out.append(f"run from={running[1][1]} to=lost task={running[0]}")
                runs.append((running[0], running[1], edge))
            for subject, since, depth, _ in isrs:
                out.append(f"interrupt from={since} to=lost depth={depth} handler={subject}")
            for mutex, (owner, since) in held.items():
                out.append(f"hold from={since} to=lost mutex={mutex} task={owner}")
            for (mutex, waiter), since in waits.items():
                out.append(f"wait from={since} to=lost mutex={mutex} task={waiter}")
            activations.extend((task, since, edge, False) for task, since in ready.items())
            seen = set(ready)
            lost, nesting_known, running, isrs, held, waits, ready = edge, False, None, [], {}, {}, {}
            continue
        at = line.split(" ")[1]
        last = (n, at)
        name = name[len(PREFIX):] if name.startswith(PREFIX) else name
        values = dict(fields)
        if name in ("task_started", "enter_idle"):
            task = values.get("task", "idle")
            if running is not None:
                out.append(f"run from={running[1][1]} to={at} task={running[0]}")
                runs.append((running[0], running[1], last))
            running = (task, last)
            if not isrs:
                nesting_known = True
        elif name == "task_ready":
            task = values["task"]
            if task in ready:
                out.append(f"contradiction {at}")
                continue
            out.append(f"wake at={at} task={task} reason={values['reason']} cause={cause(values['reason'])}")
            ready[task] = last
            seen.add(task)
        elif name in ("task_sleep", "task_wait", "task_wait_timeout"):
            end_activation(values["task"], last)
        elif name in ("isr_enter", "enter_systick", "enter_service_call"):
            depth = len(isrs) if nesting_known else "unknown"
            if name == "isr_enter":
                handler = values["handler"]
                subject = oracle.handler_subject(handler, values["exception"], names)
            else:
                handler = subject = oracle.KERNEL[name]
            isrs.append((subject, at, depth, handler))
        elif name in ("isr_exit", "exit_systick", "exit_service_call"):
            handler = oracle.KERNEL.get(name) or values["handler"]
            if isrs and isrs[-1][3] == handler:
                subject, since, depth, _ = isrs.pop()
            elif not isrs and lost is not None:
                subject, since, depth = handler, "lost", "unknown"
            else:
                out.append(f"contradiction {at}")
                continue
            out.append(f"interrupt from={since} to={at} depth={depth} handler={subject}")
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
            elif mutex not in held and lost is not None:
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
            end_activation(values["waiter"], last)
        elif name == "task_terminated":
            for mutex, waiter in [key for key in waits if key[1] == values["task"]]:
                out.append(f"wait from={waits.pop((mutex, waiter))} to={at} mutex={mutex} task={waiter}")
            end_activation(values["task"], last)
    if running is not None:
        out.append(f"run from={running[1][1]} to=open task={running[0]}")
        runs.append((running[0], running[1], last))
    activations.extend((task, since, last, True) for task, since in ready.items())
    # A task is ready in its activation but where it runs.
    by_task = {}
    for task, since, to in sorted(runs, key=lambda run: run[1][0]):
        by_task.setdefault(task, []).append((since, to))
    ends = {task: [run_to[0] for _, run_to in task_runs] for task, task_runs in by_task.items()}
    for task, since, to, is_open in activations:
        start = since
        task_runs = by_task.get(task, [])
        for run_from, run_to in task_runs[bisect.bisect_right(ends.get(task, []), since[0]):]:
            if run_from[0] > to[0]:
                break
            low = run_from if run_from[0] > since[0] else since
            if low[0] > start[0]:
                out.append(f"ready from={start[1]} to={low[1]} task={task}")
            start = run_to if run_to[0] < to[0] else to
        if to[0] > start[0]:
            out.append(f"ready from={start[1]} to={'open' if is_open else to[1]} task={task}")
    for handler, since, depth in isrs:
        out.append(f"interrupt from={since} to=open depth={depth} handler={handler}")
    for mutex, (owner, since) in held.items():
        out.append(f"hold from={since} to=open mutex={mutex} task={owner}")
    for (mutex, waiter), since in waits.items():
        out.append(f"wait from={since} to=open mutex={mutex} task={waiter}")
    return sorted(out)


def interrupt_names(host: str, image: str) -> dict[int, str]:
    """The device interrupts the image's DWARF names, as `kymo events` lists them."""
    events = subprocess.run([host, "events", image], capture_output=True, text=True, encoding="utf-8",
                            timeout=TIMEOUT_SECONDS, check=True).stdout
    return oracle.interrupt_names(events)


def check_spans(output: str, decoded: str, names: dict[int, str]) -> list[str]:
    """kymo decode --meaning's spans against those computed here, each kind seen."""
    expected, got = spans(output, names), sorted(decoded.splitlines())
    kinds = {kind: sum(line.startswith(kind + " ") for line in expected)
             for kind in ("run", "interrupt", "hold", "wait", "wake", "ready")}
    causes = {cause: sum(line.startswith("wake ") and f" cause={cause}" in line for line in expected)
              for cause in ("handler:", "task:", "elapsed")}
    granted = sum(line.startswith("wake ") and " reason=mutex_granted " in line for line in expected)
    print(f"wakes by cause: {causes}, by a mutex released: {granted}")
    print(f"spans: {kinds}, {len(got)} printed by kymo decode --meaning")
    errors = [f"expected, not printed: {line}" for line in sorted(set(expected) - set(got))]
    errors += [f"printed, not expected: {line}" for line in sorted(set(got) - set(expected))]
    if not errors and expected != got:
        errors.append("the same spans, not as many times each")
    if 0 in kinds.values():
        errors.append(f"the recording left a kind of span unexercised: {kinds}")
    if 0 in causes.values() or not granted:
        errors.append(f"the recording left a cause of wake unexercised: {causes}, mutex_granted {granted}")
    return errors


def check_stats(host: str, image: str, capture: str, durations: dict[str, int | None] = DURATIONS) -> list[str]:
    """kymo stats --json against oracle.py, on the whole capture and on its
    middle half, every kind of instance seen and some cut by the range; the
    durations those of @p durations, each its count, none cut (None: some,
    the last one perhaps cut by the end of the recording), no contradiction."""
    def run(*arguments) -> str:
        done = subprocess.run([host, *arguments], capture_output=True, text=True, encoding="utf-8", timeout=TIMEOUT_SECONDS)
        if done.returncode != 0:
            raise RuntimeError(f"kymo {arguments[0]} failed: {done.stderr.strip()}")
        return done.stdout

    try:
        names = oracle.interrupt_names(run("events", image))
        # The whole capture without a frequency: no seconds.
        decoded = run("decode", capture)
        whole = json.loads(run("stats", capture, "--json", "--worst", str(WORST)))
        # The middle half, in seconds at the frequency measured, or else at
        # the core clock the suite starts with; its bounds in cycles as kymo
        # stats computes them, rounded half away from zero.
        hz, clock, clocked = whole["frequency"], [], decoded
        if hz is None:
            hz = int(re.search(r" kymo::opsy::starting core_clock=(\d+)", decoded)[1])
            clock = ["--clock", str(hz)]
            clocked = run("decode", capture, *clock)
        origin = whole["origin"]
        bounds = [whole["from"] + (whole["to"] - whole["from"]) * quarter // 4 for quarter in (1, 3)]
        seconds = [(bound - origin) / hz for bound in bounds]
        low, high = (origin + math.floor(second * hz + 0.5) for second in seconds)
        middle = json.loads(run("stats", capture, "--json", "--worst", str(WORST), *clock,
                                "--from", repr(seconds[0]), "--to", repr(seconds[1])))
    except (RuntimeError, ValueError, KeyError, TypeError) as error:
        return [str(error)]
    errors = []
    for name, got, expected in (("whole", whole, oracle.report(decoded, None, None, WORST, names)),
                                ("middle", middle, oracle.report(clocked, low, high, WORST, names))):
        errors += [f"stats {name}: {error}" for error in oracle.compare(expected, got)]
    kinds = {kind: sum(group["count"] for group in whole["groups"] if group["kind"] == kind)
             for kind in ("task", "handler", "mutex", "duration")}
    cut = sum(group["cut"] for group in middle["groups"])
    print(f"stats: {len(whole['groups'])} groups, instances {kinds}, cycles {whole['from']} to {whole['to']}, "
          f"{cut} cut by the middle half")
    if 0 in kinds.values():
        errors.append(f"the recording left a kind of instance unexercised: {kinds}")
    # The handover begun by a task and ended by a handler after the task's
    # end pairs as the jobs within it do, without contradicting anything.
    found = {group["subject"]: (group["count"], group["cut"]) for group in whole["groups"]
             if group["kind"] == "duration"}
    expected = all(subject in found and (found[subject] == (count, 0) if count is not None
                                         else found[subject][0] > 0 and found[subject][1] <= 1)
                   for subject, count in durations.items())
    if not expected or found.keys() != durations.keys() or whole["contradictions"]:
        errors.append(f"durations (count, cut) {found} and {whole['contradictions']} contradictions, "
                      f"not {durations} and none")
    # QEMU leaves the cycle counter at 0: every duration is 0, the range cuts
    # nothing, and only the board's recording checks them.
    if whole["to"] > whole["from"] and cut == 0:
        errors.append("the middle half cut no instance")
    return errors


def check_rules(host: str, image: str, capture: str) -> list[str]:
    """kymo check against oracle.py: for each group, a bound at the oracle's
    extreme passes and one cycle past it fails; a group with excluded
    instances is unknown; the range's losses are bounded the same way; and
    every group holds within 0 % of a baseline of the same capture."""
    def run(rules: dict, folder: str) -> tuple[int, list[str]]:
        path = os.path.join(folder, "rules.json")
        with open(path, "w", encoding="utf-8") as file:
            json.dump({"baseline": "baseline.json", "rules": rules}, file)
        done = subprocess.run([host, "check", capture, path], capture_output=True, text=True, encoding="utf-8",
                              timeout=TIMEOUT_SECONDS)
        if done.returncode not in (0, 3, 4):
            raise RuntimeError(f"kymo check failed ({done.returncode}): {done.stderr.strip()}")
        return done.returncode, done.stdout.splitlines()

    def rule(group: dict, stat: str, **test) -> dict:
        written = {"kind": group["kind"], "subject": group["subject"], "metric": group["metric"], "stat": stat}
        if group["detail"] is not None:
            written["detail"] = group["detail"]
        return written | test

    def expect(name: str, rules: list[dict], code: int, label: str | None) -> list[str]:
        got, lines = run(rules, folder)
        errors = [f"check {name}: exit {got}, not {code}"] if got != code else []
        verdicts = lines[:-1]
        if len(verdicts) != len(rules):
            return errors + [f"check {name}: {len(verdicts)} verdicts for {len(rules)} rules"]
        errors += [f"check {name}: {line}" for line in verdicts if label and not line.startswith(label + " ")]
        return errors

    try:
        baseline = subprocess.run([host, "stats", capture, "--json"], capture_output=True, text=True,
                                  encoding="utf-8", timeout=TIMEOUT_SECONDS, check=True).stdout
        decoded = subprocess.run([host, "decode", capture], capture_output=True, text=True, encoding="utf-8",
                                 timeout=TIMEOUT_SECONDS, check=True).stdout
        names = interrupt_names(host, image)
    except subprocess.CalledProcessError as error:
        return [f"kymo {error.cmd[1]} failed: {error.stderr.strip()}"]
    expected = oracle.report(decoded, None, None, WORST, names)
    groups = [{"kind": kind, "subject": subject, "metric": metric, "detail": detail} | group
              for (kind, subject, metric, detail), group in expected["groups"].items()]
    known = [group for group in groups if group["distribution"] and not group["lower_bound"]]
    excluded = [group for group in groups if group["distribution"] and group["lower_bound"]]
    cycles = lambda value: f"{value}cycles"  # noqa: E731
    at, past = [], []
    for group in known:
        low, high = group["distribution"]["min"]["value"], group["distribution"]["max"]["value"]
        at += [rule(group, "max", max=cycles(high)), rule(group, "min", min=cycles(low)),
               rule(group, "count", min=group["count"], max=group["count"])]
        past += [rule(group, "max", max=cycles(high - 1)), rule(group, "min", min=cycles(low + 1)),
                 rule(group, "count", max=group["count"] - 1)]
    losses = expected["losses"]
    at.append({"capture": "losses", "min": losses, "max": losses})
    past.append({"capture": "losses", "min": losses + 1})
    unknown = [rule(group, "max", max=cycles(group["distribution"]["max"]["value"])) for group in excluded]
    regression = [rule(group, stat, regression="0%") for group in known for stat in ("max", "p50", "count")]
    with tempfile.TemporaryDirectory() as folder:
        with open(os.path.join(folder, "baseline.json"), "w", encoding="utf-8") as file:
            file.write(baseline)
        try:
            errors = expect("at the oracle's bounds", at, 0, "PASS")
            errors += expect("past the oracle's bounds", past, 3, "FAIL")
            errors += expect("against the baseline", regression, 0, "PASS")
            if unknown:
                errors += expect("on excluded groups", unknown, 4, "UNKNOWN")
                errors += expect("unknown and failed", unknown + past[:1], 3, None)
        except (RuntimeError, ValueError) as error:
            return [str(error)]
    print(f"check: {len(at)} rules at the oracle's bounds, {len(past)} past them, {len(unknown)} unknown, "
          f"{len(regression)} against the baseline")
    if not known:
        errors.append("no group without exclusions to check")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    args = parser.parse_args()

    directory = tempfile.TemporaryDirectory()
    capture = os.path.join(directory.name, "suite.kcap")
    recording = record_to_exit(args.qemu, args.machine, args.image, args.host, capture)
    stdout = recording.stdout
    print(recording.stderr.strip())
    if recording.returncode != 0 or recording.exit_status is None:
        print(stdout[-2000:])
        print("kymo record failed" if recording.returncode != 0 else f"stopped short of {EXIT}")
        return 1
    errors = check(stdout)
    decoded = subprocess.run([args.host, "decode", capture, "--meaning"],
                             capture_output=True, text=True, encoding="utf-8", timeout=TIMEOUT_SECONDS)
    if decoded.returncode != 0:
        errors.append(f"kymo decode --meaning failed: {decoded.stderr.strip()}")
    else:
        errors += check_spans(stdout, decoded.stdout, interrupt_names(args.host, args.image))
    errors += check_stats(args.host, args.image, capture)
    errors += check_rules(args.host, args.image, capture)
    directory.cleanup()
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
