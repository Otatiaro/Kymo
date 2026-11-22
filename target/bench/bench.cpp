/**
 * @file    bench.cpp
 * @brief   Throughput bench image: fills the trace ring at a rate the host
 *          sets, so the host can measure how fast a probe drains it.
 *
 * No RTOS, no vendor HAL: the core runs on its reset clock, SysTick ticks at
 * 1 kHz, and each tick writes kymo_bench_events_per_tick one-slot events.
 * The host writes that variable to step the rate; it is configuration of the
 * instrument, not of an application. Each event carries its slot index in a
 * form the host checks, so a torn read shows up as a corrupt payload.
 *
 * The same events can go out on SWO, through ITM stimulus port 1, at a rate
 * of their own: kymo_bench_swo_events_per_tick. The image does not configure
 * ITM, TPIU or the SWO pin: the host does it through the debug port, and the
 * image writes to the port only while the host keeps it enabled. Each write
 * waits for the stimulus FIFO, so SWO never drops an event: past its
 * bandwidth, the tick handler overruns and the host sees fewer events than
 * it asked for.
 */

#include <kymo/ring.hpp>

#include <cstdint>

#ifndef KYMO_BENCH_CORE_CLOCK_HZ
#error "KYMO_BENCH_CORE_CLOCK_HZ must give the core clock after reset"
#endif

#ifndef KYMO_BENCH_RING_SLOTS
#define KYMO_BENCH_RING_SLOTS 1024
#endif

namespace
{

constexpr std::uint16_t bench_event_type = 1;
constexpr std::uint32_t tick_hz = 1000;

volatile std::uint32_t& reg(std::uintptr_t address)
{
	return *reinterpret_cast<volatile std::uint32_t*>(address);
}

constexpr std::uintptr_t demcr = 0xE000EDFC;
constexpr std::uintptr_t dwt_ctrl = 0xE0001000;
constexpr std::uintptr_t dwt_cyccnt = 0xE0001004;
constexpr std::uintptr_t syst_csr = 0xE000E010;
constexpr std::uintptr_t syst_rvr = 0xE000E014;
constexpr std::uintptr_t syst_cvr = 0xE000E018;
constexpr std::uintptr_t itm_ter = 0xE0000E00;
constexpr std::uintptr_t itm_tcr = 0xE0000E80;

constexpr std::uint32_t swo_port = 1;
constexpr std::uintptr_t swo_stimulus = 0xE0000000 + 4 * swo_port;

bool swo_enabled()
{
	return (reg(itm_tcr) & 1u) != 0           // ITMENA
		&& (reg(itm_ter) & (1u << swo_port)) != 0;
}

// A disabled port never reads FIFOREADY: the wait ends when the probe turns
// ITM off, or the target would hang once it leaves.
bool swo_write(std::uint32_t word)
{
	while ((reg(swo_stimulus) & 1u) == 0)    // FIFOREADY
	{
		if (!swo_enabled())
			return false;
	}
	reg(swo_stimulus) = word;
	return true;
}

} // namespace

extern "C"
{

/// The ring the host drains. Zeroed at reset: no slot looks written.
kymo::ring<KYMO_BENCH_RING_SLOTS> kymo_ring;

/// Events written per 1 ms tick; written by the host.
volatile std::uint32_t kymo_bench_events_per_tick;

/// Milliseconds since reset, so the host can tell the image runs.
volatile std::uint32_t kymo_bench_ticks;

/// Events written to SWO per 1 ms tick; written by the host.
volatile std::uint32_t kymo_bench_swo_events_per_tick;

/// Events written to SWO since reset: the index each SWO event carries.
volatile std::uint32_t kymo_bench_swo_events;

void SysTick_Handler()
{
	kymo_bench_ticks = kymo_bench_ticks + 1;
	std::uint32_t const count = kymo_bench_events_per_tick;
	for (std::uint32_t i = 0; i < count; ++i)
	{
		// This handler is the only writer, so the index read here is the one
		// the ring hands out; the host checks the payload against it.
		std::uint32_t const index = kymo_ring.write_index.load(std::memory_order_relaxed);
		kymo_ring.write(bench_event_type, reg(dwt_cyccnt), index, ~index);
	}
	std::uint32_t const swo_count = kymo_bench_swo_events_per_tick;
	if (swo_count != 0 && swo_enabled())
	{
		std::uint32_t index = kymo_bench_swo_events;
		for (std::uint32_t i = 0; i < swo_count; ++i, ++index)
		{
			if (!(swo_write(bench_event_type) && swo_write(reg(dwt_cyccnt))
				&& swo_write(index) && swo_write(~index)))
				break;
		}
		kymo_bench_swo_events = index;
	}
}

[[noreturn]] void bench_main()
{
	reg(demcr) |= 1u << 24;   // TRCENA: DWT on
	reg(dwt_cyccnt) = 0;
	reg(dwt_ctrl) |= 1u;      // CYCCNTENA

	reg(syst_rvr) = KYMO_BENCH_CORE_CLOCK_HZ / tick_hz - 1;
	reg(syst_cvr) = 0;
	reg(syst_csr) = 0b111;    // core clock, interrupt, enable

	for (;;)
	{
	}
}

} // extern "C"
