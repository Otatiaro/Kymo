#!/usr/bin/env python3
"""Record the Zephyr test application through QEMU's monitor, and check the trace.

    trace.py --qemu Q --machine M --image I --host KYMO

Runs the application (application.cpp) under QEMU, recorded by `kymo record
--probe qemu` from its first event to its end, application_end, where the
guest halts and r0 holds its verdict. The ring holds more slots than the
application emits events: the recording loses none, and any loss fails.
Then checks that:

- the application passed;
- every event is one of the adapter's, kymo::zephyr::<hook>, with its fields;
- every thread, semaphore, message queue, mutex, event, pipe and handler is
  named from the ELF, never printed in hexadecimal;
- the idle thread starting names is one thread_create created, as "idle";
- every isr_enter names the handler of its exception, 15 the kernel's SysTick
  ISR and an interrupt the application's, and every isr_exit closes the last
  isr_enter still open, of the same exception;
- an event of the running thread, a block, a sleep or a mutex taken, released
  or blocked on, comes from a thread, never a handler;
- a thread that blocks gives the core to another: the next switch follows the
  block, with only the priority changes of a mutex's inheritance between;
- but one get from a message queue: the get that hands its slot to a writer
  blocked on the queue, which the kernel reports as blocking, is followed by
  the writer made ready, and does not block (the handoff scenario);
- a mutex is taken and released in turn, by the thread holding it, and blocked
  on by another; the recursive mutex, locked and unlocked twice, is taken and
  released once;
- every family on by default was recorded;
- the scenarios of application.cpp, in their order (SCENARIOS).
"""

import argparse
import os
import re
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "tests", "record"))
from record import record_to_exit  # noqa: E402

END = "application_end"
PREFIX = "kymo::zephyr::"
# Each event of the adapter: its family and its fields, in order.
EVENTS = {
    "starting": ("scheduling", ["core_clock", "idle"]),
    "thread_create": ("scheduling", ["thread", "priority", "name"]),
    "thread_name_set": ("scheduling", ["thread", "name"]),
    "thread_sched_abort": ("scheduling", ["thread"]),
    "thread_switched_in": ("scheduling", ["thread"]),
    "thread_sched_priority_set": ("scheduling", ["thread", "priority"]),
    "thread_sched_ready": ("causality", ["thread"]),
    "thread_sleep_ticks_enter": ("causality", ["ticks"]),
    "sem_take_blocking": ("causality", ["sem"]),
    "msgq_get_blocking": ("causality", ["msgq"]),
    "msgq_put_blocking": ("causality", ["msgq"]),
    "event_wait_blocking": ("causality", ["event"]),
    "pipe_read_blocking": ("causality", ["pipe"]),
    "pipe_write_blocking": ("causality", ["pipe"]),
    "mutex_lock_blocking": ("exclusion", ["mutex"]),
    "mutex_taken": ("exclusion", ["mutex"]),
    "mutex_released": ("exclusion", ["mutex"]),
    "isr_enter": ("interrupts", ["exception", "handler"]),
    "isr_exit": ("interrupts", ["exception"]),
}
DEFAULT_FAMILIES = {"scheduling", "causality", "interrupts", "exclusion"}
POINTERS = {"thread", "idle", "sem", "msgq", "event", "pipe", "mutex", "handler"}
MUTEX_EVENTS = {"mutex_taken", "mutex_released", "mutex_lock_blocking"}
# Where the running thread blocks: the core goes to another thread.
BLOCKS = {"thread_sleep_ticks_enter", "sem_take_blocking", "msgq_get_blocking", "msgq_put_blocking",
          "event_wait_blocking", "pipe_read_blocking", "pipe_write_blocking", "mutex_lock_blocking"}
# Events of the running thread, which a handler never emits.
OF_RUNNING = BLOCKS | MUTEX_EVENTS
SYSTICK, SYSTICK_HANDLER = "15", "sys_clock_isr"
DEVICE_HANDLER = "application::device_handler"
FIRST_IRQ = 16

CONTROL, IDLE = "z_main_thread", "z_idle_threads"
PING, PONG, RECEIVER = "_k_thread_obj_ping", "_k_thread_obj_pong", "_k_thread_obj_receiver"
WRITER, LOW, EPHEMERAL = "_k_thread_obj_writer", "_k_thread_obj_low", "application::ephemeral"
FREE, RECURSIVE = "application::free_mutex", "application::recursive_mutex"
CONTESTED = "application::contested_mutex"
ROUNDS = 3  # application::rounds


def switch(thread: str) -> tuple:
    return ("thread_switched_in", {"thread": thread})


def ready(thread: str) -> tuple:
    return ("thread_sched_ready", {"thread": thread})


# A tick ends a sleep or a timed wait of control's: the idle thread runs until
# SysTick makes control ready.
TICK = [switch(IDLE), ("isr_enter", {"exception": SYSTICK}), ready(CONTROL),
        ("isr_exit", {"exception": SYSTICK}), switch(CONTROL)]

# What each scenario emits, in order: (event, the fields it must have). Other
# events may come between them.
SCENARIOS = [
    ("start", [("starting", {}),
               ("thread_create", {"thread": CONTROL, "priority": "3", "name": '"main"'}),
               ("thread_create", {"thread": IDLE, "name": '"idle"'}),
               switch(CONTROL),
               ("thread_create", {"thread": LOW, "priority": "5", "name": '"low"'}),
               ("thread_create", {"thread": PING, "priority": "2", "name": '"ping"'}),
               ("thread_create", {"thread": PONG, "priority": "2", "name": '"pong"'}),
               ("thread_create", {"thread": RECEIVER, "priority": "1", "name": '"receiver"'}),
               ("thread_create", {"thread": WRITER, "priority": "4", "name": '"writer"'})]),
    ("switches", [ready(PING), switch(PING)]
     + [ready(PONG), ("sem_take_blocking", {"sem": "application::ping_sem"}), switch(PONG),
        ready(PING), ("sem_take_blocking", {"sem": "application::pong_sem"}), switch(PING)] * ROUNDS
     + [("sem_take_blocking", {"sem": "application::ping_sem"}), switch(CONTROL)]),
    ("handler", [("isr_enter", {"handler": DEVICE_HANDLER}), ready(RECEIVER), ("isr_exit", {}),
                 switch(RECEIVER), ("msgq_get_blocking", {"msgq": "application::handler_msgq"}),
                 switch(CONTROL)]),
    ("mutexes", [("mutex_taken", {"mutex": FREE}), ("mutex_released", {"mutex": FREE}),
                 ("mutex_taken", {"mutex": RECURSIVE}), ("mutex_released", {"mutex": RECURSIVE}),
                 ("sem_take_blocking", {"sem": "application::control_sem"}), switch(LOW),
                 ("mutex_taken", {"mutex": CONTESTED}), ready(CONTROL), switch(CONTROL),
                 ("mutex_lock_blocking", {"mutex": CONTESTED}),
                 ("thread_sched_priority_set", {"thread": LOW, "priority": "3"}), switch(LOW),
                 ("mutex_released", {"mutex": CONTESTED}),
                 ("thread_sched_priority_set", {"thread": LOW, "priority": "5"}),
                 ready(CONTROL), switch(CONTROL),
                 ("mutex_taken", {"mutex": CONTESTED}), ("mutex_released", {"mutex": CONTESTED})]),
    ("sleeps", [("thread_sleep_ticks_enter", {"ticks": "1"})] + TICK),
    ("ready priority", [ready(LOW), ("thread_sched_priority_set", {"thread": LOW, "priority": "4"}),
                        ("thread_sched_priority_set", {"thread": LOW, "priority": "5"}),
                        ("thread_sleep_ticks_enter", {"ticks": "1"}), switch(LOW),
                        ("sem_take_blocking", {"sem": "application::low_sem"})] + TICK),
    ("waits", [("sem_take_blocking", {"sem": "application::empty_sem"})] + TICK
     + [("msgq_get_blocking", {"msgq": "application::empty_msgq"})] + TICK
     + [("msgq_put_blocking", {"msgq": "application::full_msgq"})] + TICK
     + [("event_wait_blocking", {"event": "application::event"})] + TICK
     + [("pipe_read_blocking", {"pipe": "application::pipe"})] + TICK
     + [("pipe_write_blocking", {"pipe": "application::pipe"})] + TICK),
    ("handoff", [ready(WRITER), ("thread_sleep_ticks_enter", {"ticks": "1"}), switch(WRITER),
                 ("msgq_put_blocking", {"msgq": "application::handoff_msgq"})] + TICK
     + [("msgq_get_blocking", {"msgq": "application::handoff_msgq"}), ready(WRITER)]),
    ("creation", [("thread_create", {"thread": EPHEMERAL, "priority": "4", "name": '""'}),
                  ("thread_name_set", {"thread": EPHEMERAL, "name": '"ephemeral"'}),
                  ("thread_sched_abort", {"thread": EPHEMERAL})]),
]


def parse(output: str) -> tuple[list, list[str]]:
    """The events, (line, name, fields), and the lines that say events were lost."""
    events, losses = [], []
    for line in output.splitlines():
        words = line.split(" ", 5)
        if (words[0] == "record" and int(words[1]) != 0) or words[0] == "lost":
            losses.append(line)
        elif words[0] == "event":
            # A name may hold a space, "(anonymous namespace)": a field
            # starts where a word is followed by "=".
            fields = re.split(r" (?=\w+=)", words[5]) if len(words) > 5 else []
            events.append((line, words[4], [tuple(field.split("=", 1)) for field in fields]))
    return events, losses


def check_scenarios(events: list) -> list[str]:
    """SCENARIOS, in order, a subsequence of the events."""
    at = 0
    for scenario, expected in SCENARIOS:
        for name, fields in expected:
            while at < len(events) and not (
                    events[at][0] == name and fields.items() <= events[at][1].items()):
                at += 1
            if at == len(events):
                return [f"scenario {scenario}: no {name} {fields} where expected"]
            at += 1
    return []


def check(output: str, core_clock: str = "25000000") -> list[str]:
    """The events against the adapter's and the scenarios', `starting`
    giving core_clock, the clock of the board's SysTick."""
    errors = []
    events, losses = parse(output)
    errors += [f"{line}: the ring holds every event, none may be lost" for line in losses]
    checked = []  # (name, values) of the adapter's events
    families = set()
    created = {}  # thread -> its name
    idle = None
    running = None
    open_isrs = []
    blocked = None  # (line, name, thread) of the last block, until the next switch
    handoffs = 0
    holders = {}  # mutex -> the thread holding it
    taken = {}  # mutex -> the times it was taken
    for line, name, fields in events:
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
        checked.append((name, values))
        for field in POINTERS & values.keys():
            if values[field].startswith("0x"):
                errors.append(f"{line}: {field} is not named")
        if name in OF_RUNNING and open_isrs:
            errors.append(f"{line}: emitted by a handler, not the running thread")

        if blocked is not None and name != "thread_switched_in":
            block_line, block, _ = blocked
            if block == "msgq_get_blocking" and name == "thread_sched_ready" and not open_isrs:
                handoffs += 1   # the get handed its slot to the writer, and goes on
                blocked = None
            elif name != "thread_sched_priority_set":
                errors.append(f"{block_line}: blocks, and {name} comes before the switch")
                blocked = None
        if name in BLOCKS and not open_isrs:
            blocked = (line, name, running)

        if name == "thread_create":
            created[values["thread"]] = values["name"]
        elif name == "thread_name_set":
            if values["thread"] not in created:
                errors.append(f"{line}: a thread never created")
            created[values["thread"]] = values["name"]
        elif name == "starting":
            if values["core_clock"] != core_clock:
                errors.append(f"{line}: not the core clock {core_clock}")
            idle = values["idle"]
        elif name == "thread_switched_in":
            if values["thread"] not in created:
                errors.append(f"{line}: a thread never created")
            if blocked is not None and values["thread"] == blocked[2]:
                errors.append(f"{line}: {blocked[2]} blocked, and kept the core")
            running, blocked = values["thread"], None
        elif name == "isr_enter":
            exception = int(values["exception"])
            handler = SYSTICK_HANDLER if values["exception"] == SYSTICK else DEVICE_HANDLER
            if exception != int(SYSTICK) and exception < FIRST_IRQ or values["handler"] != handler:
                errors.append(f"{line}: not the handler of exception {values['exception']}")
            open_isrs.append(values["exception"])
        elif name == "isr_exit":
            if not open_isrs or open_isrs[-1] != values["exception"]:
                errors.append(f"{line}: closes no open isr_enter of its exception")
            else:
                open_isrs.pop()
        elif name == "mutex_taken":
            mutex = values["mutex"]
            if mutex in holders:
                errors.append(f"{line}: {holders[mutex]} holds it")
            holders[mutex] = running
            taken[mutex] = taken.get(mutex, 0) + 1
        elif name == "mutex_released":
            if holders.get(values["mutex"]) != running:
                errors.append(f"{line}: released by {running}, not its holder {holders.get(values['mutex'])}")
            holders.pop(values["mutex"], None)
        elif name == "mutex_lock_blocking":
            if holders.get(values["mutex"]) in (None, running):
                errors.append(f"{line}: blocked on by its holder, or a free mutex")

    if idle is None or created.get(idle) != '"idle"':
        errors.append("the idle thread starting names was not created as \"idle\"")
    if handoffs != 1:
        errors.append(f"{handoffs} gets handing their slot to a writer, not the handoff scenario's one")
    if taken.get(RECURSIVE) != 1:
        errors.append(f"the recursive mutex taken {taken.get(RECURSIVE, 0)} times, not once")
    for family in sorted(DEFAULT_FAMILIES - families):
        errors.append(f"no event of the family {family}")
    errors += check_scenarios(checked)
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--host", required=True)
    args = parser.parse_args()

    with tempfile.TemporaryDirectory() as directory:
        capture = os.path.join(directory, "application.kcap")
        recording = record_to_exit(args.qemu, args.machine, args.image, args.host, capture, END)
    print(recording.stderr.strip())
    if recording.returncode != 0 or recording.exit_status is None:
        print(recording.stdout[-2000:])
        print("kymo record failed" if recording.returncode != 0 else f"stopped short of {END}")
        return 1
    errors = [] if recording.exit_status == 1 else ["the application failed"]
    errors += check(recording.stdout)
    for error in errors[:20]:
        print(error)
    print("RESULT:", "FAIL" if errors else "PASS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
