# Kymo

An open-source flight recorder for Cortex-M real-time systems: **the target
writes, the ELF describes, the host understands.**

Kymo records what an RTOS does (which task runs and why it woke up,
interrupts, mutexes) with as little intrusion as possible, and turns the
recording into a timeline, statistics and rule checks on the host.

> **Status: early work.** The target library, the OpSy and FreeRTOS adapters,
> recording through a probe, decoding, statistics, rule checks and the timeline
> work; formats still change without notice.

## Principles

- **Data flows from the target to the host only.** The host reads the trace
  through the debug port and never touches the application. The one word
  meant for it to write is the control word, which families are recorded
  (`target/include/kymo/control.hpp`); `kymo` does not write it yet, and the
  program sets it with `kymo::enable()`, `kymo::disable()` and
  `kymo::freeze()`.
- **The ELF is the static information.** Each event type is a plain
  `struct` on the target; the host reads its layout from DWARF and names
  tasks, objects and interrupts from symbols. Events carry addresses, not
  names, except where the kernel only knows a name at run time (a FreeRTOS
  task's, in `task_create`).
- **Lock-free on the target.** Overwrite rings of fixed-size slots, one per
  channel the application routes event families to, reserved with
  `LDREX`/`STREX`; interrupts are never masked; losses are counted exactly,
  and a write overrun by a full ring lap is reported as suspect.
- **The ring outlives the program.** It is plain RAM: a copy dumped from a
  stopped or crashed target decodes like a recording.
- **Measured.** Every design choice on the target is judged by its measured
  cost in cycles, flash and RAM; the adapters' cost per kernel path is held
  against a record in CI.

## Quick start

On the target, include `target/include` (C++17) and declare a tracer that
routes each event family to a channel; a ring is the channel the host drains:

```cpp
#include <kymo/kymo.hpp>

inline kymo::ring<256> os;

struct trace : kymo::tracer<trace>
{
    template<kymo::family F>
    static auto& channel(kymo::on<F>) { return os; }
};

struct sample { std::uint16_t channel; std::int32_t value; };

trace::emit(sample{3, -42});
```

An event type may declare a text, which the host writes in place of its
fields. It names each field, with `std::format`'s specifications; it is read
from the DWARF and costs the target nothing:

```cpp
struct moved
{
    mode from;
    mode to;
    float load;
    static constexpr char kymo_text[] = "{from} -> {to} at {load:.1f}%";
};
```

A field is an integer, a `bool`, a `char`, a floating-point number, an
enumeration (written by name) or a `char` array (written up to its first NUL);
`{{` and `}}` write a brace. `kymo events` refuses an image whose text names a
field the event lacks, or gives a field a specification its type does not take.
Nothing in the program reads the text: in an unnamed namespace, Clang warns
that it is unused, which `[[maybe_unused]]` silences.

An event type is also a duration. `trace::begin` opens one with the event's
fields and `trace::end<Event>()` closes the last one of that type still open;
`trace::scope` closes it at the end of a block. Begin and end may come from
any context (a task, a handler, another task): the host pairs them, not the
target. Durations that overlap in any order take a key, which the event's text
names `{kymo_key}`:

```cpp
struct batch
{
    std::uint32_t id;
    static constexpr char kymo_text[] = "batch {id} in buffer {kymo_key}";
};

trace::begin(batch{7}, buffer);   // in the producer
trace::end<batch>(buffer);        // in the consumer's handler
```

`kymo stats` and `kymo check` give each type its `duration`. An end with
nothing open, a key opened twice or closed while not open is counted as a
contradiction; a duration still open when the recording ends is cut.

Each numeric field of an application event is a curve: an integer of 1, 2, 4
or 8 bytes, signed or not, or a `float` or `double`; a `bool`, a `char`, an
enumeration, an array and kymo's own events are not. Each emission is a
sample. `kymo stats` gives each curve its `value` group, named
`<event> value <field>`: the count, the NaN apart, min, percentiles, max and
mean; `kymo check` bounds them with plain numbers:

```json
{ "subject": "moved", "metric": "value", "detail": "load", "stat": "max", "max": 95.0 }
```

`kymo-ui` shows none at first: its Values menu ticks the curves to show, and
`--value <event.field>` one at launch. Each is a line under the durations, a
step from each sample to the next, cut by a loss, none where a sample is NaN,
its axis from the lowest to the highest value in the view. Zoomed out, each
pixel column spans the step entering it and its samples, so a peak of one
sample stays visible; zoomed in, each sample is a dot. Hovering shows the
value held, when it was sampled and, with an RTOS adapter, by which task or
handler.

Neither costs more than `emit`: a begin is the emit of the event's fields, a
keyed begin of the key and the fields, an end of nothing, a keyed end of the
key, to the instruction and the byte (`target/tests/cost`, in CI). A text is
never in what the target loads (`target/tests/no_text.py`), as long as the
program does not use it.

The program enables the DWT cycle counter, which timestamps the events. An
RTOS adapter emits the kernel's events through the same tracer:
`target/opsy/include/kymo/opsy/hooks.hpp` for OpSy,
`target/freertos/include/kymo/freertos/trace.h` for FreeRTOS; each header
says how to install it.

The OpSy adapter records a hardware fault as a `kymo::fault`
(`target/include/kymo/fault.hpp`): the fault status registers, the frame the
core stacked and `EXC_RETURN`, before the application's fault handler runs.
With them, the frame's address, the bounds of the stack it should be on (the
faulting task's, from `stack_base()` and `stack_size()`, or the main stack's,
from the vector table) and up to eight words above the frame that may be
return addresses, read from at most 64 words of that stack: the host makes the
first call levels from them. A frame outside its stack is recorded, its stack
not read. It does not freeze the trace: an interrupt of higher priority than the fault
still writes, and overwrites what came before. A handler that keeps the board
for the host to read calls `kymo::freeze()` first.

FreeRTOS has no fault hook: the application's fault handler calls
`kymo_freertos_fault()` with the frame and `EXC_RETURN` (`trace.h` shows the
entry stub). The bounds of a task's stack are the kernel's, unknown there: on
the process stack the 64 words are read whatever they are, so every task stack
must start at least 256 bytes before the end of its readable memory.

On the host, build with `cargo build --release --manifest-path host/Cargo.toml`
(add `-p kymo-ui` for the timeline), then:

```sh
kymo events app.elf                     # the event types the ELF declares
kymo record --probe probe-rs --chip STM32H563ZITx --elf app.elf --out run.kymo
kymo decode run.kymo                    # the events, one line each
kymo decode run.kymo --meaning          # the kernel's spans and wakes
kymo stats run.kymo [--json]            # latencies, durations, periods, shares, values
kymo check run.kymo rules.json          # rules on those statistics, an exit code for CI
kymo-ui [run.kymo]                      # the timeline; open, drop or paste the URL of a capture
kymo-ui run.kymo --value moved.load     # with that curve shown
kymo mcp                                # an MCP server on stdio: captures read by an agent
```

`kymo mcp` serves an agent through the Model Context Protocol: each tool takes
a capture's path and answers text: `summary`, `find`, `events`, `context` (what
is open at an instant), `subject` (a task's activations, a handler's runs),
`intervals` (from an event to the next of another), `stats` and `check`.

> [!WARNING]
> A capture holds the ELF given to `record`: the program's code, its symbols,
> its debug information and the paths of its sources. Sending a capture sends
> all of it. To share one without the ELF, record it with `--elf-ref`.

The capture holds the ELF given to `record`, so it reads back on its own.
`record --elf-ref <file or URL>` names the ELF instead of holding it, and
`--elf` on the other commands reads a capture with another ELF. An ELF named by an
http(s) URL is fetched, and `kymo-ui` opens a capture from its URL too.

`kymo` with no argument prints every command and option. The JSON of
`stats --json` and the rules of `check` are described by
`host/schema/stats.schema.json` and `host/schema/check.schema.json`.

## Layout

| Path | Content |
|---|---|
| `target/` | the target side, C++17: core and one adapter per RTOS ([OpSy](https://github.com/Otatiaro/OpSy), [FreeRTOS](https://github.com/FreeRTOS/FreeRTOS-Kernel)), with their tests under QEMU |
| `host/` | the host side, Rust: acquisition (probe-rs, J-Link and ST-Link libraries, QEMU), model, the `kymo` CLI |
| `host/ui/` | `kymo-ui`, the timeline of a capture (egui on wgpu), apart so the CLI pulls no graphics |

Both sides evolve in the same commit: the event format is the contract between
them.

## Targets and probes

Cortex-M33 and Cortex-M4 first, then M3 and M7. Probes: SEGGER J-Link and
ST-Link (V2-1, V3), through probe-rs and through the vendors' own libraries
where they bring speed or features. Host: Windows and Linux, x86-64 and ARM64.

On an STM32H5, record through probe-rs, not through the ST-Link library:
STM32CubeProgrammer, attaching to an H5 even in hot-plug mode, halts the core
and disables every interrupt, so the program being recorded stops.

## License

[MIT](LICENSE). `kymo-ui` embeds the Inter font, under the SIL Open Font
License 1.1 ([`host/ui/fonts/Inter-LICENSE.txt`](host/ui/fonts/Inter-LICENSE.txt)).
