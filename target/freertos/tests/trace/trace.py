#!/usr/bin/env python3
"""Record the FreeRTOS test application through QEMU's monitor, and check the trace.

    trace.py --qemu Q --machine M --image I --host KYMO

Runs the application (application.cpp) under QEMU, recorded by `kymo record
--probe qemu` from its first event to its end, application_end, where the
guest halts and r0 holds its verdict. The ring holds more slots than the
application emits events: the recording loses none, and any loss fails.
Then checks that:

- the application passed;
- every event is one of the adapter's, kymo::freertos::<macro>, with its
  fields;
- every task, queue, mutex, event group, stream buffer and handler is named
  from the ELF, never printed in hexadecimal;
- the idle task starting names is one task_create created, as "IDLE";
- every isr_enter names the handler of its exception, 15 the port's SysTick
  handler and 16 the application's, and every isr_exit closes the last
  isr_enter still open, of the same exception;
- an event of the running task, a block, a delay or a mutex taken, released
  or blocked on, comes from a task, never a handler; a task that blocks
  gives the core to another, the next task_switched_in;
- a mutex is taken and released in turn, by the task holding it, and blocked
  on by another; a mutex is never seen as a queue: no queue event names an
  object a mutex event names; the recursive mutex, taken and given twice, is
  taken and released once;
- task_priority_inherit raises the holder's priority, task_priority_disinherit
  drops it back to the one task_create or task_priority_set gave it;
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
PREFIX = "kymo::freertos::"
# Each event of the adapter: its family and its fields, in order.
EVENTS = {
    "starting": ("scheduling", ["core_clock", "idle"]),
    "task_create": ("scheduling", ["task", "priority", "name"]),
    "task_delete": ("scheduling", ["task"]),
    "task_switched_in": ("scheduling", ["task"]),
    "task_priority_set": ("scheduling", ["task", "priority"]),
    "task_priority_inherit": ("scheduling", ["task", "priority"]),
    "task_priority_disinherit": ("scheduling", ["task", "priority"]),
    "moved_task_to_ready_state": ("causality", ["task"]),
    "task_delay": ("causality", ["ticks"]),
    "task_delay_until": ("causality", ["time"]),
    "task_suspend": ("causality", ["task"]),
    "task_resume": ("causality", ["task"]),
    "blocking_on_queue_receive": ("causality", ["queue"]),
    "blocking_on_queue_send": ("causality", ["queue"]),
    "blocking_on_queue_peek": ("causality", ["queue"]),
    "task_notify_take_block": ("causality", ["index"]),
    "task_notify_wait_block": ("causality", ["index"]),
    "event_group_sync_block": ("causality", ["group"]),
    "event_group_wait_bits_block": ("causality", ["group"]),
    "blocking_on_stream_buffer_send": ("causality", ["stream"]),
    "blocking_on_stream_buffer_receive": ("causality", ["stream"]),
    "queue_send": ("causality", ["queue"]),
    "task_notify": ("causality", ["task"]),
    "mutex_taken": ("exclusion", ["mutex"]),
    "mutex_released": ("exclusion", ["mutex"]),
    "mutex_blocked": ("exclusion", ["mutex"]),
    "isr_enter": ("interrupts", ["exception", "handler"]),
    "isr_exit": ("interrupts", ["exception"]),
}
DEFAULT_FAMILIES = {"scheduling", "causality", "interrupts", "exclusion"}
POINTERS = {"task", "idle", "queue", "mutex", "group", "stream", "handler"}
QUEUE_EVENTS = {"queue_send", "blocking_on_queue_receive", "blocking_on_queue_send", "blocking_on_queue_peek"}
MUTEX_EVENTS = {"mutex_taken", "mutex_released", "mutex_blocked"}
# Where the running task blocks: the core goes to another task.
BLOCKS = {"task_delay", "task_delay_until", "blocking_on_queue_receive", "blocking_on_queue_send",
          "blocking_on_queue_peek", "task_notify_take_block", "task_notify_wait_block",
          "event_group_sync_block", "event_group_wait_bits_block", "blocking_on_stream_buffer_send",
          "blocking_on_stream_buffer_receive", "mutex_blocked"}
# Events of the running task, which a handler never emits.
OF_RUNNING = BLOCKS | MUTEX_EVENTS
# The handler of each exception: SysTick, the port's, named per port; the
# device interrupt, the application's.
HANDLERS = {"15": {"xPortSysTickHandler", "SysTick_Handler"}, "16": {"application::device_handler"}}

CONTROL, PING, PONG = "application::control_tcb", "application::ping_tcb", "application::pong_tcb"
RECEIVER, LOW, EPHEMERAL = "application::receiver_tcb", "application::low_tcb", "application::ephemeral_tcb"
FREE, RECURSIVE = "application::free_mutex_storage", "application::recursive_mutex_storage"
CONTESTED = "application::contested_mutex_storage"
HANDLER_QUEUE, EMPTY, FULL = ("application::handler_queue_storage", "application::empty_queue_storage",
                              "application::full_queue_storage")
GROUP, STREAM = "application::group_storage", "application::stream_storage"
ROUNDS = 3  # application::rounds

# What each scenario emits, in order: (event, the fields it must have). Other
# events may come between them.
SCENARIOS = [
    ("start", [("task_create", {"task": CONTROL, "priority": "3", "name": '"control"'}),
               ("task_create", {"task": PING, "priority": "4", "name": '"ping"'}),
               ("task_create", {"task": PONG, "priority": "4", "name": '"pong"'}),
               ("task_create", {"task": RECEIVER, "priority": "5", "name": '"receiver"'}),
               ("task_create", {"task": LOW, "priority": "1", "name": '"low"'}),
               ("starting", {"core_clock": "25000000"}),
               ("task_switched_in", {"task": CONTROL})]),
    ("switches", [("task_notify", {"task": PING}), ("task_switched_in", {"task": PING})]
     + [("task_notify", {"task": PONG}), ("task_switched_in", {"task": PONG}),
        ("task_notify", {"task": PING}), ("task_switched_in", {"task": PING})] * ROUNDS
     + [("task_switched_in", {"task": CONTROL})]),
    ("handler", [("isr_enter", {"exception": "16", "handler": "application::device_handler"}),
                 ("queue_send", {"queue": HANDLER_QUEUE}),
                 ("moved_task_to_ready_state", {"task": RECEIVER}),
                 ("isr_exit", {"exception": "16"}),
                 ("task_switched_in", {"task": RECEIVER}),
                 ("blocking_on_queue_receive", {"queue": HANDLER_QUEUE}),
                 ("task_switched_in", {"task": CONTROL})]),
    ("mutexes", [("mutex_taken", {"mutex": FREE}), ("mutex_released", {"mutex": FREE}),
                 ("mutex_taken", {"mutex": RECURSIVE}), ("mutex_released", {"mutex": RECURSIVE}),
                 ("task_notify", {"task": LOW}), ("task_switched_in", {"task": LOW}),
                 ("mutex_taken", {"mutex": CONTESTED}),
                 ("task_notify", {"task": CONTROL}), ("task_switched_in", {"task": CONTROL}),
                 ("mutex_blocked", {"mutex": CONTESTED}),
                 ("task_priority_inherit", {"task": LOW, "priority": "3"}),
                 ("task_switched_in", {"task": LOW}),
                 ("mutex_released", {"mutex": CONTESTED}),
                 ("task_priority_disinherit", {"task": LOW, "priority": "1"}),
                 ("task_switched_in", {"task": CONTROL}),
                 ("mutex_taken", {"mutex": CONTESTED}), ("mutex_released", {"mutex": CONTESTED})]),
    ("delays", [("task_delay", {"ticks": "1"}),
                ("isr_enter", {"exception": "15"}), ("moved_task_to_ready_state", {"task": CONTROL}),
                ("isr_exit", {"exception": "15"}), ("task_switched_in", {"task": CONTROL}),
                ("task_delay_until", {}), ("task_switched_in", {"task": CONTROL})]),
    ("ready priority", [("task_notify", {"task": LOW}),
                        ("task_priority_set", {"task": LOW, "priority": "2"}),
                        ("moved_task_to_ready_state", {"task": LOW}),
                        ("task_priority_set", {"task": LOW, "priority": "1"}),
                        ("moved_task_to_ready_state", {"task": LOW}),
                        ("task_delay", {"ticks": "1"}), ("task_switched_in", {"task": CONTROL})]),
    ("suspension", [("task_suspend", {"task": LOW}), ("task_resume", {"task": LOW}),
                    ("moved_task_to_ready_state", {"task": LOW}), ("task_delay", {"ticks": "1"}),
                    ("task_switched_in", {"task": CONTROL})]),
    ("waits", [("blocking_on_queue_peek", {"queue": EMPTY}),
               ("blocking_on_queue_receive", {"queue": EMPTY}),
               ("queue_send", {"queue": FULL}), ("blocking_on_queue_send", {"queue": FULL}),
               ("task_notify_take_block", {"index": "0"}),
               ("task_notify_wait_block", {"index": "0"}),
               ("event_group_wait_bits_block", {"group": GROUP}),
               ("event_group_sync_block", {"group": GROUP}),
               ("blocking_on_stream_buffer_receive", {"stream": STREAM}),
               ("blocking_on_stream_buffer_send", {"stream": STREAM})]),
    ("creation", [("task_create", {"task": EPHEMERAL, "priority": "2", "name": '"ephemeral"'}),
                  ("task_delete", {"task": EPHEMERAL})]),
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


def check(output: str) -> list[str]:
    errors = []
    events, losses = parse(output)
    errors += [f"{line}: the ring holds every event, none may be lost" for line in losses]
    checked = []  # (name, values) of the adapter's events
    families = set()
    created = {}  # task -> its name
    priorities = {}  # task -> (base, current)
    running = None
    open_isrs = []
    blocked = None  # the task that last blocked, until the next switch
    holders = {}  # mutex -> the task holding it
    taken = {}  # mutex -> the times it was taken
    mutexes, queues = set(), set()
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
            errors.append(f"{line}: emitted by a handler, not the running task")
        if name in BLOCKS and not open_isrs:
            blocked = running
        if name in MUTEX_EVENTS:
            mutexes.add(values["mutex"])
        if name in QUEUE_EVENTS:
            queues.add(values["queue"])

        if name == "task_create":
            created[values["task"]] = values["name"]
            priorities[values["task"]] = (int(values["priority"]),) * 2
        elif name == "starting":
            if created.get(values["idle"]) != '"IDLE"':
                errors.append(f"{line}: the idle task was not created as \"IDLE\"")
        elif name == "task_switched_in":
            if values["task"] not in created:
                errors.append(f"{line}: a task never created")
            if blocked is not None and values["task"] == blocked:
                errors.append(f"{line}: {blocked} blocked, and kept the core")
            running, blocked = values["task"], None
        elif name == "task_priority_set":
            priorities[values["task"]] = (int(values["priority"]),) * 2
        elif name == "task_priority_inherit":
            base, current = priorities.get(values["task"], (None, None))
            if current is not None and int(values["priority"]) <= current:
                errors.append(f"{line}: inherits no higher priority than its {current}")
            priorities[values["task"]] = (base, int(values["priority"]))
        elif name == "task_priority_disinherit":
            base, _ = priorities.get(values["task"], (None, None))
            if base is not None and int(values["priority"]) != base:
                errors.append(f"{line}: drops to {values['priority']}, not its base {base}")
            priorities[values["task"]] = (base, base)
        elif name == "isr_enter":
            if values["handler"] not in HANDLERS.get(values["exception"], set()):
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
        elif name == "mutex_blocked":
            if holders.get(values["mutex"]) in (None, running):
                errors.append(f"{line}: blocked on by its holder, or a free mutex")

    for queue in sorted(queues & mutexes):
        errors.append(f"{queue}: a mutex seen as a queue")
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
        recording = record_to_exit(args.qemu, args.machine, args.image, args.host,
                                   os.path.join(directory, "application.kcap"), END)
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
