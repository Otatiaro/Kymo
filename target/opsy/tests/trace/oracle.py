"""kymo stats recomputed from the event lines of kymo decode, apart from the host.

    names = oracle.interrupt_names(events)  # what `kymo events <elf>` prints
    expected = oracle.report(decoded, low, high, worst, names, oracle.vectors(elf))
    errors = oracle.compare(expected, json.loads(stats_json))

`decoded` is what `kymo decode <capture>` prints: the record line, then
an element per line (event, lost, suspect), then, when the SWO dates
exceptions, its span `swo <first> <last>`, its gaps `gap <from> <to>` and
its packets `exception <cycles> <number> entered|exited|returned`, then the
end line. The elements are numbered in that order; an edge is (element,
cycles, lost).

What makes an instance, from OpSy's events (ADR-062):

- an activation runs from a task's task_ready to its task_sleep, task_wait,
  task_wait_timeout, mutex_blocked or task_terminated; its response time is
  all of it; its wake latency, to the task's first run in it, in total, by the
  reason of the task_ready and by its cause; its gross execution, from that run to its
  end; its net execution, its runs less the outermost handlers within them;
  the time preempted by tasks, the gross less the runs, and by handlers, the
  outermost handlers within the runs. An activation the task never runs in
  has only a response time;
- a task_ready's cause is `elapsed` for a delay that ran out; else the
  innermost handler open but the kernel's, `handler:<name>`; else the task
  running, `task:<name>` or `task:idle`; `unknown` from a loss to the next
  task_started or enter_idle;
- a task's period runs from a task_ready to its next;
- a handler's subject is `<handler> (<interrupt>)`, the interrupt named by
  its exception number; an exit closes the innermost handler entered, of
  its address, or after a loss names the handler alone;
- a handler's gross runs from its entry to its exit; its net is less the
  handlers nested one level within it; its period runs from an entry seen to
  the next;
- with the SWO (ADR-104), its packets nest per stretch of its coverage, and
  each exception's handler takes the subject `<interrupt> (<handler>)`, the
  handler the adapter enters for it, or the vector table's with a `?`
  (take_swo). The adapter's spans of that handler within the coverage give
  way to the SWO's, and give their costs `adapter_entry` and `adapter_exit`;
  its others keep their place under the SWO's subject. An SWO instance
  counts unless a bound is lost or a gap of the SWO starts within it; it is
  dated by its cycles, at the frequency of the events' seconds;
- a mutex's hold runs from mutex_taken to mutex_released, its wait from
  mutex_blocked to the waiter's mutex_taken or task_terminated; each in total
  and by task.

What makes a duration of the application's, of no context of OpSy's
(ADR-088): an end_of<T> closes the last begin_of<T> still open, a
keyed_end_of<T> the keyed_begin_of<T> of its kymo_key. An end that closes
nothing, and a key begun while open, are contradictions; the later begin
replaces the earlier.

A loss ends what is open at it, and what ends after it with no start seen
starts at it; a duration's end with no begin seen after it is no
contradiction, and makes no instance. What is still open at the last event
ends there, open.

Over the range [low, high) in cycles: an instance inside counts, unless it
touches a loss or covers an overrun write; one across an edge of the range,
or open, is cut; one outside is left out.
"""

import bisect
import math
import re
import struct

PREFIX = "kymo::opsy::"
ENDS = {"task_sleep", "task_wait", "task_wait_timeout"}
ELAPSED = {"timed_out", "sleep_elapsed"}
KERNEL = {"enter_systick": "systick", "exit_systick": "systick",
          "enter_service_call": "service_call", "exit_service_call": "service_call"}
KERNEL_EXCEPTIONS = {"enter_systick": 15, "enter_service_call": 11}
# The exceptions the architecture names, by number; the device's interrupts
# start at FIRST_IRQ.
ARCHITECTURE = {1: "Reset", 2: "NMI", 3: "HardFault", 4: "MemManage", 5: "BusFault", 6: "UsageFault",
                7: "SecureFault", 11: "SVCall", 12: "DebugMonitor", 14: "PendSV", 15: "SysTick"}
FIRST_IRQ = 16
DURATION = re.compile(r"kymo::(keyed_)?(begin|end)_of<(.+)>")


def interrupt_names(events: str) -> dict[int, str]:
    """The device interrupts `kymo events` lists, by exception number."""
    return {int(words[1]): words[2] for words in (line.split() for line in events.splitlines())
            if len(words) == 3 and words[0] == "interrupt"}


def handler_subject(handler: str, exception: str, names: dict[int, str]) -> str:
    """The subject of a handler entered for an exception: `<handler> (<interrupt>)`,
    or the handler alone when called from thread mode (0) or named after it."""
    number = int(exception, 0)
    if number == 0:
        return handler
    interrupt = interrupt_name(number, names)
    return handler if interrupt == handler else f"{handler} ({interrupt})"


def interrupt_name(number: int, names: dict[int, str]) -> str:
    """The name of exception `number`: the architecture's, or the device's."""
    if number in ARCHITECTURE:
        return ARCHITECTURE[number]
    if number < FIRST_IRQ:
        return f"exception{number}"
    return names.get(number, f"IRQ{number - FIRST_IRQ}")


def vectors(image: str) -> dict[int, str]:
    """The functions the vector table of the ELF `image` gives, by exception
    number: the table is the first word aligned on 128 bytes of a loaded
    section that is a stack pointer, non-zero and word-aligned, followed by
    the entry with its Thumb bit; a vector names the function that starts
    at it."""
    with open(image, "rb") as file:
        data = file.read()
    entry, shoff = struct.unpack_from("<I", data, 0x18)[0], struct.unpack_from("<I", data, 0x20)[0]
    shentsize, shnum = struct.unpack_from("<HH", data, 0x2E)
    # name, type, flags, addr, offset, size, link, info, addralign, entsize
    sections = [struct.unpack_from("<10I", data, shoff + n * shentsize) for n in range(shnum)]
    functions = {}
    for _, kind, _, _, offset, size, link, _, _, _ in sections:
        if kind == 2:  # SHT_SYMTAB
            strings = sections[link][4]
            for at in range(offset, offset + size, 16):
                name, value, _, info = struct.unpack_from("<IIIB", data, at)
                if info & 0xF == 2:  # STT_FUNC
                    start = strings + name
                    functions.setdefault(value & ~1, data[start:data.index(b"\0", start)].decode())
    for _, kind, flags, address, offset, size, _, _, _, _ in sections:
        if kind != 1 or not flags & 2:  # SHT_PROGBITS, SHF_ALLOC
            continue
        for at in range(-address % 128, size - 7, 128):
            stack, reset = struct.unpack_from("<II", data, offset + at)
            if stack != 0 and stack % 4 == 0 and reset == entry | 1:
                words = struct.unpack_from(f"<{min((size - at) // 4, 16 + 496)}I", data, offset + at)
                return {number: functions[word & ~1] for number, word in enumerate(words)
                        if number != 0 and word & ~1 in functions}
    return {}


def elements(decoded: str) -> tuple[int, list[dict]]:
    """The first index recorded, and the elements in order."""
    first, out = 0, []
    for line in decoded.splitlines():
        words = line.split(" ", 4)
        if words[0] == "record":
            first = int(words[1])
        elif words[0] == "event":
            # A name or a value may hold a space: a field starts where a word
            # is followed by "=".
            name, *fields = re.split(r" (?=\w+=)", words[4])
            # One index, or the range of the event's slots: "11439-11440".
            out.append({"kind": "event", "index": int(words[1].split("-")[0]), "cycles": int(words[2]),
                        "seconds": None if words[3] == "-" else float(words[3]),
                        "name": name.removeprefix(PREFIX),
                        "values": dict(field.split("=", 1) for field in fields)})
        elif words[0] == "lost":
            out.append({"kind": "lost"})
        elif words[0] == "suspect":
            out.append({"kind": "suspect", "from": int(words[1]), "to": int(words[2]), "writers": int(words[3])})
    return first, out


def spans(first: int, items: list[dict], names: dict[int, str]) -> dict:
    """OpSy's activations, wakes, runs, interrupts, holds and contradictions;
    a handler's subject names its interrupt from `names` (handler_subject)."""
    found = {"activations": [], "wakes": [], "runs": [], "interrupts": [], "holds": [], "durations": [],
             "contradictions": [], "traced": {}}
    running = None  # (task or None for idle, since), None while unknown
    begun = {}  # (type, key or None): [since], the last begun last
    entered = []  # (subject, since, handler), innermost last
    held = {}  # mutex: (task, since)
    waiting = {}  # (mutex, task): since
    ready = {}  # task: (since, reason, cause)
    seen = set()  # tasks whose activation is known since the last loss
    loss = None
    last = None

    def end_activation(task, edge):
        if task in ready:
            since, reason, cause = ready.pop(task)
        elif loss is not None and task not in seen:
            since, reason, cause = loss, None, None
        else:
            since = None
        seen.add(task)
        if since is not None:
            found["activations"].append((task, reason, cause, since, edge, False))

    def cause_of(reason):
        if reason in ELAPSED:
            return "elapsed"
        handlers = [subject for subject, _, handler in entered if handler not in KERNEL.values()]
        if handlers:
            return "handler:" + handlers[-1]
        if running is None:
            return "unknown"
        return "task:" + (running[0] or "idle")

    def close_all(edge, is_open):
        if running is not None:
            found["runs"].append((running[0], running[1], edge, is_open))
        for depth, (subject, since, handler) in enumerate(entered):
            found["interrupts"].append((subject, depth, since, edge, is_open, handler))
        for mutex, (task, since) in held.items():
            found["holds"].append((mutex, task, False, since, edge, is_open))
        for (mutex, task), since in waiting.items():
            found["holds"].append((mutex, task, True, since, edge, is_open))
        for task, (since, reason, cause) in ready.items():
            found["activations"].append((task, reason, cause, since, edge, is_open))
        for (of, _), stack in begun.items():
            for since in stack:
                found["durations"].append((of, since, edge, is_open))

    def duration(n, edge, match, values):
        keyed, begins, of = match.groups()
        key = (of, values["kymo_key"] if keyed else None)
        if begins == "begin":
            stack = begun.setdefault(key, [])
            if keyed and stack:
                found["contradictions"].append(n)
                stack.clear()
            stack.append(edge)
        elif begun.get(key):
            found["durations"].append((of, begun[key].pop(), edge, False))
        elif loss is None:
            found["contradictions"].append(n)

    for n, item in enumerate(items):
        if item["kind"] == "suspect":
            continue
        # A loss, or the first event of a recording that started late.
        hidden = None
        if item["kind"] == "lost":
            hidden = (n, last[1] if last else 0, True)
        elif last is None and loss is None and first != 0:
            hidden = (n, item["cycles"], True)
        if hidden is not None:
            close_all(hidden, False)
            seen = set(ready)
            running, entered, held, waiting, ready, begun, loss = None, [], {}, {}, {}, {}, hidden
        if item["kind"] == "lost":
            continue
        edge = (n, item["cycles"], False)
        last = edge
        name, values = item["name"], item["values"]
        match = DURATION.fullmatch(name)
        if match:
            duration(n, edge, match, values)
        elif name in ("task_started", "enter_idle"):
            if running is not None:
                found["runs"].append((running[0], running[1], edge, False))
            running = (values.get("task"), edge)
        elif name == "task_ready":
            task = values["task"]
            if task in ready:
                found["contradictions"].append(n)
                continue
            ready[task] = (edge, values["reason"], cause_of(values["reason"]))
            seen.add(task)
            found["wakes"].append((task, edge))
        elif name in ENDS:
            end_activation(values["task"], edge)
        elif name == "task_terminated":
            task = values["task"]
            for key in [key for key in waiting if key[1] == task]:
                found["holds"].append((key[0], task, True, waiting.pop(key), edge, False))
            end_activation(task, edge)
        elif name == "isr_enter":
            handler, number = values["handler"], int(values["exception"], 0)
            subject = handler_subject(handler, values["exception"], names)
            if number != 0:
                found["traced"].setdefault(number, (subject, handler))
            entered.append((subject, edge, handler))
        elif name in ("enter_systick", "enter_service_call"):
            found["traced"].setdefault(KERNEL_EXCEPTIONS[name], (KERNEL[name], KERNEL[name]))
            entered.append((KERNEL[name], edge, KERNEL[name]))
        elif name in ("isr_exit", "exit_systick", "exit_service_call"):
            # An exit closes the innermost handler entered, of its address.
            handler = KERNEL.get(name) or values["handler"]
            if entered and entered[-1][2] == handler:
                subject, since, _ = entered.pop()
            elif not entered and loss is not None:
                subject, since = handler, loss
            else:
                found["contradictions"].append(n)
                continue
            found["interrupts"].append((subject, len(entered), since, edge, False, handler))
        elif name == "mutex_taken":
            mutex, task = values["mutex"], values["owner"]
            if mutex in held:
                found["contradictions"].append(n)
                continue
            if (mutex, task) in waiting:
                found["holds"].append((mutex, task, True, waiting.pop((mutex, task)), edge, False))
            held[mutex] = (task, edge)
        elif name == "mutex_released":
            mutex, task = values["mutex"], values["owner"]
            if mutex in held and held[mutex][0] == task:
                since = held.pop(mutex)[1]
            elif mutex not in held and loss is not None:
                since = loss
            else:
                found["contradictions"].append(n)
                continue
            found["holds"].append((mutex, task, False, since, edge, False))
        elif name == "mutex_blocked":
            mutex, task = values["mutex"], values["waiter"]
            if (mutex, task) in waiting:
                found["contradictions"].append(n)
                continue
            waiting[(mutex, task)] = edge
            end_activation(task, edge)
    if last is not None:
        close_all(last, True)
    return found


def swo(decoded: str) -> tuple[list[tuple], list[tuple], list[tuple]]:
    """The stretches of the SWO's coverage, (from, to) in cycles: from its
    first anchor to its last, less its gaps; the gaps; and its exception
    packets, (cycles, number, function), in the order of the stream."""
    span, gaps, packets = None, [], []
    for words in (line.split() for line in decoded.splitlines()):
        if words[:1] == ["swo"]:
            span = (int(words[1]), int(words[2]))
        elif words[:1] == ["gap"]:
            gaps.append((int(words[1]), int(words[2])))
        elif words[:1] == ["exception"]:
            packets.append((int(words[1]), int(words[2]), words[3]))
    covered = []
    if span is not None:
        start = span[0]
        for low, high in gaps:
            if low > start:
                covered.append((start, low))
            start = max(start, high)
        if span[1] > start:
            covered.append((start, span[1]))
    return covered, gaps, packets


def nest(covered: list[tuple], packets: list[tuple]) -> tuple[list[tuple], list[int]]:
    """The handlers' spans the packets give, (number, from, to), each bound
    (cycles, lost), and the cycles of the packets that contradict them. Within
    each stretch apart: an entry opens a handler, an exit closes the innermost
    one, or one entered before the stretch while none is known open; a return
    to thread mode with handlers open contradicts them and makes the open ones
    known from then on; a return to another names the innermost one. What is
    open at a stretch's end ends there, lost."""
    lines, contradictions = [], []
    cycles = [packet[0] for packet in packets]
    for start, end in covered:
        opened, known = [], False
        for at, number, function in packets[bisect.bisect_left(cycles, start):bisect.bisect_right(cycles, end)]:
            if function == "entered":
                opened.append((number, (at, False)))
            elif function == "exited":
                if opened and opened[-1][0] == number:
                    lines.append((number, opened.pop()[1], (at, False)))
                elif not opened and not known:
                    lines.append((number, (start, True), (at, False)))
                else:
                    contradictions.append(at)
            elif number == 0:
                if opened:
                    contradictions.append(at)
                opened, known = [], True
            elif opened[-1][0] != number if opened else known:
                contradictions.append(at)
        lines += [(number, since, (end, True)) for number, since in reversed(opened)]
    return lines, contradictions


def take_swo(found: dict, items: list[dict], decoded: str, names: dict[int, str], vectors: dict[int, str]):
    """The SWO's handlers in `found`. Each exception's subject is
    `<interrupt> (<handler>)`: the handler the adapter enters for it, or the
    vector table's with a `?`. A handler the adapter traces for an exception
    loses its spans that touch the coverage to the SWO's, and keeps the
    others under the SWO's subject; the SWO's span with no lost bound that
    holds such a span lost gives the adapter's costs, the SWO's entry to the
    adapter's, and the adapter's exit to the SWO's. An SWO edge falls after
    the last event at or before its cycles, or at element 0."""
    covered, gaps, packets = swo(decoded)
    found["costs"], found["swo"] = [], {"subjects": set(), "gaps": [low for low, _ in gaps]}
    interrupts, found["interrupts"] = found["interrupts"], []
    if not packets:
        found["interrupts"] = [interrupt[:5] for interrupt in interrupts]
        return
    events = [(item["cycles"], n) for n, item in enumerate(items) if item["kind"] == "event"]
    event_cycles = [cycles for cycles, _ in events]

    def edge(at):
        before = bisect.bisect_right(event_cycles, at[0])
        return (events[before - 1][1] if before else 0, at[0], at[1])

    traced = found["traced"]
    doubly = {handler for _, handler in traced.values()}

    def exception_of(subject, handler):
        numbers = [n for n, (of, _) in traced.items() if of == subject] \
            or [n for n, (_, of) in traced.items() if of == handler]
        return min(numbers, default=None)

    def subject_of(number):
        interrupt = interrupt_name(number, names)
        handler = traced[number][1] if number in traced else vectors.get(number)
        mark = "" if number in traced else "?"
        subject = interrupt if handler is None or handler == interrupt else f"{interrupt} ({handler}{mark})"
        found["swo"]["subjects"].add(subject)
        return subject

    lines, contradictions = nest(covered, packets)
    found["contradictions"] += [edge((at, False))[0] for at in contradictions]
    replaced = []
    for subject, depth, since, to, is_open, handler in interrupts:
        if handler in doubly and any(since[1] <= high and to[1] >= low for low, high in covered):
            replaced.append((subject, since, to, is_open, handler))
            continue
        number = exception_of(subject, handler) if handler in doubly else None
        found["interrupts"].append((subject if number is None else subject_of(number), depth, since, to, is_open))
    found["interrupts"] += [(subject_of(number), None, edge(since), edge(to), False) for number, since, to in lines]
    whole = sorted((number, since[0], since, to) for number, since, to in lines if not since[1] and not to[1])
    keys = [(number, at) for number, at, _, _ in whole]
    for subject, since, to, is_open, handler in replaced:
        number = exception_of(subject, handler)
        if is_open or since[2] or to[2] or number is None:
            continue
        holding = bisect.bisect_right(keys, (number, since[1]))
        if holding and keys[holding - 1][0] == number and whole[holding - 1][3][0] >= to[1]:
            _, _, swo_from, swo_to = whole[holding - 1]
            found["costs"].append((subject_of(number), edge(swo_from), since, to, edge(swo_to)))


def at(edge) -> tuple[int, int]:
    """Where an edge is: by element, and by cycles within one, where the SWO dates its edges."""
    return edge[0], edge[1]


def clipped(span_from, span_to, low, high):
    """The part of the span [span_from, span_to] within [low, high], as edges."""
    start = span_from if at(span_from) > at(low) else low
    end = span_to if at(span_to) < at(high) else high
    return start, end


def instances(found: dict) -> list[tuple]:
    """(kind, subject, metric, detail, value, from, to, open) of every instance."""
    out = []
    # Spans of one sort never overlap: sorted by start, their ends are sorted too.
    runs = {}
    for task, since, to, _ in sorted(found["runs"], key=lambda run: run[1][0]):
        runs.setdefault(task, []).append((since, to))
    # A handler's parent is the innermost one whose span holds its entry; a
    # handler whose entry a loss hid holds those that run before its exit.
    interrupts = sorted(found["interrupts"], key=lambda interrupt: (at(interrupt[2]), -interrupt[3][0], -interrupt[3][1]))
    parents = []
    holding = []  # indexes of the interrupts whose span holds the current entry
    for n, (_, _, since, _, _) in enumerate(interrupts):
        holding = [m for m in holding if at(interrupts[m][3]) > at(since)]
        parents.append(holding[-1] if holding else None)
        holding.append(n)
    outermost = [(since, to) for (_, _, since, to, _), parent in zip(interrupts, parents) if parent is None]
    outermost_ends = [at(to) for _, to in outermost]

    def handled(low, high):
        total = 0
        for since, to in outermost[bisect.bisect_right(outermost_ends, at(low)):]:
            if at(since) >= at(high):
                break
            start, end = clipped(since, to, low, high)
            total += end[1] - start[1]
        return total

    run_ends = {task: [to[0] for _, to in spans] for task, spans in runs.items()}
    for task, reason, cause, since, to, is_open in found["activations"]:
        def add(metric, value, detail=None):
            out.append(("task", task, metric, detail, value, since, to, is_open))
        add("response", to[1] - since[1])
        windows = []
        for run_from, run_to in runs.get(task, [])[bisect.bisect_right(run_ends.get(task, []), since[0]):]:
            if run_from[0] > to[0]:
                break
            windows.append(clipped(run_from, run_to, since, to))
        if not windows:
            continue
        ran = sum(end[1] - start[1] for start, end in windows)
        by_handlers = sum(handled(start, end) for start, end in windows)
        gross = to[1] - windows[0][0][1]
        latency = windows[0][0][1] - since[1]
        add("wake_latency", latency)
        if reason is not None:
            add("wake_latency", latency, reason)
        if cause is not None:
            add("wake_latency", latency, cause)
        add("preempted_by_tasks", gross - ran)
        add("preempted_by_handlers", by_handlers)
        add("net", ran - by_handlers)
        add("gross", gross)
    wakes = sorted(found["wakes"], key=lambda wake: (wake[0], wake[1][0]))
    for (task, since), (next_task, to) in zip(wakes, wakes[1:]):
        if task == next_task:
            out.append(("task", task, "period", None, to[1] - since[1], since, to, False))

    nets = [to[1] - since[1] for _, _, since, to, _ in interrupts]
    for n, parent in enumerate(parents):
        if parent is not None:
            since, to = interrupts[n][2], interrupts[n][3]
            nets[parent] -= to[1] - since[1]
    for net, (handler, _, since, to, is_open) in zip(nets, interrupts):
        out.append(("handler", handler, "gross", None, to[1] - since[1], since, to, is_open))
        out.append(("handler", handler, "net", None, net, since, to, is_open))
    entries = sorted(((handler, since) for handler, _, since, _, _ in interrupts if not since[2]),
                     key=lambda entry: (entry[0], entry[1][0]))
    for (handler, since), (next_handler, to) in zip(entries, entries[1:]):
        if handler == next_handler:
            out.append(("handler", handler, "period", None, to[1] - since[1], since, to, False))
    for handler, swo_from, since, to, swo_to in found["costs"]:
        out.append(("handler", handler, "adapter_entry", None, since[1] - swo_from[1], swo_from, since, False))
        out.append(("handler", handler, "adapter_exit", None, swo_to[1] - to[1], to, swo_to, False))

    for mutex, task, waits, since, to, is_open in found["holds"]:
        metric = "wait" if waits else "hold"
        out.append(("mutex", mutex, metric, None, to[1] - since[1], since, to, is_open))
        out.append(("mutex", mutex, metric, task, to[1] - since[1], since, to, is_open))

    for of, since, to, is_open in found["durations"]:
        out.append(("duration", of, "duration", None, to[1] - since[1], since, to, is_open))
    return out


def percentile(values: list[int], percent: int) -> int:
    return values[max(math.ceil(percent * len(values) / 100), 1) - 1]


def histogram(values: list[int]) -> dict:
    low, high = values[0], values[-1]
    width = 1
    while high // width - low // width >= 32:
        width = {1: 2, 2: 5, 5: 10}[int(str(width)[0])] * 10 ** (len(str(width)) - 1)
    counts = [0] * (high // width - low // width + 1)
    for value in values:
        counts[value // width - low // width] += 1
    return {"width": width, "first": low // width * width, "counts": counts}


def report(decoded: str, low: int | None, high: int | None, worst: int, names: dict[int, str],
           vectors: dict[int, str] | None = None) -> dict:
    """What kymo stats --json gives over [low, high) in cycles, none for no
    bound; `names` names the interrupts (interrupt_names), `vectors` the
    handlers of the vector table (vectors)."""
    first, items = elements(decoded)
    found = spans(first, items, names)
    take_swo(found, items, decoded, names, vectors or {})
    events = [item for item in items if item["kind"] == "event"]
    origin = events[0]["cycles"] if events else 0
    # The frequency of the seconds, from the last event the seconds date.
    hz = next((event["cycles"] - origin) / event["seconds"] for event in reversed(events)
              if event["seconds"] and event["cycles"] != origin) if any(event["seconds"] for event in events) else None
    last = events[-1]["cycles"] if events else 0
    low = -2 ** 63 if low is None else low
    high = 2 ** 63 - 1 if high is None else high
    start = max(low, origin)
    end = max(min(high, last), start)

    def inside(cycles):
        return low <= cycles < high

    # Each element's place: its event's cycles and index, or the event's before it.
    place, before = [], (origin, events[0]["index"] if events else first)
    for item in items:
        if item["kind"] == "event":
            before = (item["cycles"], item["index"])
        place.append(before)
    lost = [n for n, item in enumerate(items) if item["kind"] == "lost"]
    # A recording that started late joined the trace: not a loss.
    losses = sum(inside(place[n][0]) for n in lost)
    # The time known: while the task on the core is, in a run.
    known = sum(max(min(to[1], end) - max(since[1], start), 0) for _, since, to, _ in found["runs"])
    suspects = sum(item["writers"] for n, item in enumerate(items)
                   if item["kind"] == "suspect" and inside(place[n][0]))
    overruns = [(item["from"], item["to"]) for item in items if item["kind"] == "suspect"]

    groups = {}
    for kind, subject, metric, detail, value, since, to, is_open in instances(found):
        whole = not is_open and since[1] >= low and to[1] <= high
        if not whole and not (since[1] < high and to[1] >= low):
            continue
        group = groups.setdefault((kind, subject, metric, detail),
                                  {"count": 0, "excluded_loss": 0, "excluded_suspect": 0, "cut": 0, "points": []})
        low_index, high_index = place[since[0]][1], place[to[0]][1]
        if not whole:
            group["cut"] += 1
        elif kind == "handler" and subject in found["swo"]["subjects"]:
            # A gap of the SWO drops the packets in it: a span across one may miss an entry.
            if since[2] or to[2] or any(since[1] <= gap < to[1] for gap in found["swo"]["gaps"]):
                group["excluded_loss"] += 1
            else:
                group["count"] += 1
                group["points"].append({"value": value, "element": since[0], "index": low_index,
                                        "cycles": since[1], "seconds": (since[1] - origin) / hz if hz else None})
        elif since[2] or to[2] or bisect.bisect_left(lost, to[0]) > bisect.bisect_right(lost, since[0]):
            group["excluded_loss"] += 1
        elif any(lo <= high_index and hi > low_index for lo, hi in overruns):
            group["excluded_suspect"] += 1
        else:
            group["count"] += 1
            group["points"].append({"value": value, "element": since[0], "index": low_index,
                                    "cycles": since[1], "seconds": items[since[0]].get("seconds")})
    for (kind, subject, metric, detail), group in groups.items():
        points = sorted(group.pop("points"), key=lambda point: (point["value"], point["element"]))
        group["lower_bound"] = group["excluded_loss"] + group["excluded_suspect"] > 0
        group["distribution"] = None
        if points:
            values = [point["value"] for point in points]
            highest = sorted(points, key=lambda point: (-point["value"], point["element"]))
            group["distribution"] = {
                "min": points[0], "p50": percentile(values, 50), "p90": percentile(values, 90),
                "p99": percentile(values, 99), "max": highest[0], "mean": sum(values) / len(values),
                "total": sum(values), "histogram": histogram(values), "worst": highest[:worst]}
        total = group["distribution"]["total"] if points else 0
        group["jitter"] = values[-1] - values[0] if metric == "period" and points else None
        group["share"] = total / known if metric == "net" and known > 0 else None
    return {"from": start, "to": end, "origin": origin, "known": known, "joined": first or None, "losses": losses,
            "suspects": suspects, "contradictions": sum(inside(place[n][0]) for n in found["contradictions"]),
            "groups": groups}


def close(a, b) -> bool:
    if a is None or b is None:
        return a is b
    return math.isclose(a, b, rel_tol=1e-9, abs_tol=1e-8)


def compare(expected: dict, got: dict) -> list[str]:
    """kymo stats --json against the oracle, value by value; the curves'
    groups aside, which values.py checks against its own."""
    errors = []
    for key in ("from", "to", "origin", "known", "joined", "losses", "suspects", "contradictions"):
        if got[key] != expected[key]:
            errors.append(f"{key}: kymo stats says {got[key]}, the oracle {expected[key]}")
    printed = {(group["kind"], group["subject"], group["metric"], group["detail"]): group for group in got["groups"]
               if group["kind"] != "value"}
    for key in sorted(set(expected["groups"]) | set(printed), key=str):
        if key not in printed:
            errors.append(f"{key}: not printed")
            continue
        if key not in expected["groups"]:
            errors.append(f"{key}: printed, not expected")
            continue
        want, have = expected["groups"][key], printed[key]
        for field in ("count", "excluded_loss", "excluded_suspect", "cut", "lower_bound", "jitter"):
            if want[field] != have[field]:
                errors.append(f"{key} {field}: {have[field]}, expected {want[field]}")
        if not close(want["share"], have["share"]):
            errors.append(f"{key} share: {have['share']}, expected {want['share']}")
        want, have = want["distribution"], have["distribution"]
        if (want is None) != (have is None):
            errors.append(f"{key} distribution: {have}, expected {want}")
            continue
        if want is None:
            continue
        for field in ("p50", "p90", "p99", "total", "histogram"):
            if want[field] != have[field]:
                errors.append(f"{key} {field}: {have[field]}, expected {want[field]}")
        if not close(want["mean"], have["mean"]):
            errors.append(f"{key} mean: {have['mean']}, expected {want['mean']}")
        pairs = [("min", want["min"], have["min"]), ("max", want["max"], have["max"])]
        if len(want["worst"]) != len(have["worst"]):
            errors.append(f"{key} worst: {len(have['worst'])} instances, expected {len(want['worst'])}")
        pairs += [(f"worst {n}", w, h) for n, (w, h) in enumerate(zip(want["worst"], have["worst"]))]
        for name, w, h in pairs:
            if (w["value"], w["index"], w["cycles"]) != (h["value"], h["index"], h["cycles"]) \
                    or not close(w["seconds"], h["seconds"]):
                errors.append(f"{key} {name}: {h}, expected {w}")
    return errors
