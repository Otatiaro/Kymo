"""kymo stats recomputed from the event lines of kymo decode, apart from the host.

    expected = oracle.report(decoded, low, high, worst)
    errors = oracle.compare(expected, json.loads(stats_json))

`decoded` is what `kymo decode <elf> <capture>` prints: the record line, then
an element per line (event, lost, suspect), then the end line. The elements
are numbered in that order; an edge is (element, cycles, lost).

What makes an instance, from OpSy's events (ADR-062):

- an activation runs from a task's task_ready to its task_sleep, task_wait,
  task_wait_timeout, mutex_blocked or task_terminated; its response time is
  all of it; its wake latency, to the task's first run in it, in total and by
  the reason of the task_ready; its gross execution, from that run to its
  end; its net execution, its runs less the outermost handlers within them;
  the time preempted by tasks, the gross less the runs, and by handlers, the
  outermost handlers within the runs. An activation the task never runs in
  has only a response time;
- a task's period runs from a task_ready to its next;
- a handler's gross runs from its entry to its exit; its net is less the
  handlers nested one level within it; its period runs from an entry seen to
  the next;
- a mutex's hold runs from mutex_taken to mutex_released, its wait from
  mutex_blocked to the waiter's mutex_taken or task_terminated; each in total
  and by task.

A loss ends what is open at it, and what ends after it with no start seen
starts at it. What is still open at the last event ends there, open.

Over the range [low, high) in cycles: an instance inside counts, unless it
touches a loss or covers an overrun write; one across an edge of the range,
or open, is cut; one outside is left out.
"""

import bisect
import math
import re

PREFIX = "kymo::opsy::"
ENDS = {"task_sleep", "task_wait", "task_wait_timeout"}
KERNEL = {"enter_systick": "systick", "exit_systick": "systick",
          "enter_service_call": "service_call", "exit_service_call": "service_call"}


def elements(decoded: str) -> tuple[int, list[dict]]:
    """The first index recorded, and the elements in order."""
    first, out = 0, []
    for line in decoded.splitlines():
        words = line.split(" ", 5)
        if words[0] == "record":
            first = int(words[1])
        elif words[0] == "event":
            fields = re.split(r" (?=\w+=)", words[5]) if len(words) > 5 else []
            out.append({"kind": "event", "index": int(words[1]), "cycles": int(words[2]),
                        "seconds": None if words[3] == "-" else float(words[3]),
                        "name": words[4].removeprefix(PREFIX),
                        "values": dict(field.split("=", 1) for field in fields)})
        elif words[0] == "lost":
            out.append({"kind": "lost"})
        elif words[0] == "suspect":
            out.append({"kind": "suspect", "from": int(words[1]), "to": int(words[2]), "writers": int(words[3])})
    return first, out


def spans(first: int, items: list[dict]) -> dict:
    """OpSy's activations, wakes, runs, interrupts, holds and contradictions."""
    found = {"activations": [], "wakes": [], "runs": [], "interrupts": [], "holds": [], "contradictions": []}
    running = None  # (task or None for idle, since), None while unknown
    entered = []  # (handler, since), innermost last
    held = {}  # mutex: (task, since)
    waiting = {}  # (mutex, task): since
    ready = {}  # task: (since, reason)
    seen = set()  # tasks whose activation is known since the last loss
    loss = None
    last = None

    def end_activation(task, edge):
        if task in ready:
            since, reason = ready.pop(task)
        elif loss is not None and task not in seen:
            since, reason = loss, None
        else:
            since = None
        seen.add(task)
        if since is not None:
            found["activations"].append((task, reason, since, edge, False))

    def close_all(edge, is_open):
        if running is not None:
            found["runs"].append((running[0], running[1], edge, is_open))
        for depth, (handler, since) in enumerate(entered):
            found["interrupts"].append((handler, depth, since, edge, is_open))
        for mutex, (task, since) in held.items():
            found["holds"].append((mutex, task, False, since, edge, is_open))
        for (mutex, task), since in waiting.items():
            found["holds"].append((mutex, task, True, since, edge, is_open))
        for task, (since, reason) in ready.items():
            found["activations"].append((task, reason, since, edge, is_open))

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
            running, entered, held, waiting, ready, loss = None, [], {}, {}, {}, hidden
        if item["kind"] == "lost":
            continue
        edge = (n, item["cycles"], False)
        last = edge
        name, values = item["name"], item["values"]
        if name in ("task_started", "enter_idle"):
            if running is not None:
                found["runs"].append((running[0], running[1], edge, False))
            running = (values.get("task"), edge)
        elif name == "task_ready":
            task = values["task"]
            if task in ready:
                found["contradictions"].append(n)
                continue
            ready[task] = (edge, values["reason"])
            seen.add(task)
            found["wakes"].append((task, edge))
        elif name in ENDS:
            end_activation(values["task"], edge)
        elif name == "task_terminated":
            task = values["task"]
            for key in [key for key in waiting if key[1] == task]:
                found["holds"].append((key[0], task, True, waiting.pop(key), edge, False))
            end_activation(task, edge)
        elif name in ("isr_enter", "enter_systick", "enter_service_call"):
            entered.append((KERNEL.get(name) or values["handler"], edge))
        elif name in ("isr_exit", "exit_systick", "exit_service_call"):
            handler = KERNEL.get(name) or values["handler"]
            if entered and entered[-1][0] == handler:
                since = entered.pop()[1]
            elif not entered and loss is not None:
                since = loss
            else:
                found["contradictions"].append(n)
                continue
            found["interrupts"].append((handler, len(entered), since, edge, False))
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


def clipped(span_from, span_to, low, high):
    """The part of the span [span_from, span_to] within [low, high], by element, as edges."""
    start = span_from if span_from[0] > low[0] else low
    end = span_to if span_to[0] < high[0] else high
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
    interrupts = sorted(found["interrupts"], key=lambda interrupt: (interrupt[2][0], -interrupt[3][0]))
    parents = []
    holding = []  # indexes of the interrupts whose span holds the current entry
    for n, (_, _, since, _, _) in enumerate(interrupts):
        holding = [m for m in holding if interrupts[m][3][0] > since[0]]
        parents.append(holding[-1] if holding else None)
        holding.append(n)
    outermost = [(since, to) for (_, _, since, to, _), parent in zip(interrupts, parents) if parent is None]
    outermost_ends = [to[0] for _, to in outermost]

    def handled(low, high):
        total = 0
        for since, to in outermost[bisect.bisect_right(outermost_ends, low[0]):]:
            if since[0] >= high[0]:
                break
            start, end = clipped(since, to, low, high)
            total += end[1] - start[1]
        return total

    run_ends = {task: [to[0] for _, to in spans] for task, spans in runs.items()}
    for task, reason, since, to, is_open in found["activations"]:
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

    for mutex, task, waits, since, to, is_open in found["holds"]:
        metric = "wait" if waits else "hold"
        out.append(("mutex", mutex, metric, None, to[1] - since[1], since, to, is_open))
        out.append(("mutex", mutex, metric, task, to[1] - since[1], since, to, is_open))
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


def report(decoded: str, low: int | None, high: int | None, worst: int) -> dict:
    """What kymo stats --json gives over [low, high) in cycles, none for no bound."""
    first, items = elements(decoded)
    found = spans(first, items)
    events = [item for item in items if item["kind"] == "event"]
    origin = events[0]["cycles"] if events else 0
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
    losses = int(first != 0 and low <= origin) + sum(inside(place[n][0]) for n in lost)
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
    return {"from": start, "to": end, "origin": origin, "known": known, "losses": losses,
            "suspects": suspects, "contradictions": sum(inside(place[n][0]) for n in found["contradictions"]),
            "groups": groups}


def close(a, b) -> bool:
    if a is None or b is None:
        return a is b
    return math.isclose(a, b, rel_tol=1e-9, abs_tol=1e-8)


def compare(expected: dict, got: dict) -> list[str]:
    """kymo stats --json against the oracle, value by value."""
    errors = []
    for key in ("from", "to", "origin", "known", "losses", "suspects", "contradictions"):
        if got[key] != expected[key]:
            errors.append(f"{key}: kymo stats says {got[key]}, the oracle {expected[key]}")
    printed = {(group["kind"], group["subject"], group["metric"], group["detail"]): group for group in got["groups"]}
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
