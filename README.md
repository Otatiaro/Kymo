# Kymo

An open-source flight recorder for Cortex-M real-time systems: **the target
writes, the ELF describes, the host understands.**

Kymo records what an RTOS does (which task runs and why it woke up, interrupts,
mutexes, faults) with as little intrusion as possible, keeps working after a
crash, and turns the recording into a timeline and metrics on the host.

> **Status: early work.** Nothing is usable yet. The first step is a throughput
> bench: how fast each debug probe can drain a trace ring from a running target.

## Principles

- **Data flows from the target to the host only.** The host configures the
  instrument (filter, freeze), never the application.
- **The ELF is the only source of static information.** Each event type is a
  plain `struct` on the target; the host reads its layout from DWARF, resolves
  objects and source lines from symbols, and checks the build-id. The target
  sends no strings.
- **Lock-free on the target.** A single overwrite ring of fixed-size slots,
  reserved with `LDREX`/`STREX`; interrupts are never masked; losses are
  counted exactly.
- **A crash is an event.** What happens next (freeze, reboot, wait for the host)
  is the application's choice.
- **Measured.** Every design choice on the target is judged by its measured
  cost in cycles, flash and RAM.

## Layout

| Path | Content |
|---|---|
| `target/` | the target side, C++17: core and one adapter per RTOS ([OpSy](https://github.com/Otatiaro/OpSy) first) |
| `host/` | the host side, Rust: acquisition (probe-rs, J-Link and ST-Link libraries), model, outputs (CLI, MCP, UI) |

Both sides evolve in the same commit: the event format is the contract between
them.

## Targets and probes

Cortex-M33 and Cortex-M4 first, then M3 and M7. Probes: SEGGER J-Link and
ST-Link (V2-1, V3), through probe-rs and through the vendors' own libraries
where they bring speed or features. Host: Windows and Linux.

## License

[MIT](LICENSE).
