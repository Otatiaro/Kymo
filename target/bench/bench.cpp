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

#include <atomic>
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

/// The bench event: its slot index, and its complement.
struct bench_event
{
	std::uint32_t index;
	std::uint32_t complement;
};
constexpr std::uint32_t tick_hz = 1000;

volatile std::uint32_t& reg(std::uintptr_t address)
{
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): a register at a fixed address
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

constexpr std::uint32_t demcr_trcena = 1U << 24;
constexpr std::uint32_t dwt_ctrl_cyccntena = 1U;
constexpr std::uint32_t syst_csr_enable = 1U;
constexpr std::uint32_t syst_csr_tickint = 1U << 1;
constexpr std::uint32_t syst_csr_clksource = 1U << 2;
constexpr std::uint32_t itm_tcr_itmena = 1U;
constexpr std::uint32_t itm_stimulus_fifoready = 1U;

constexpr std::uint32_t swo_port = 1;
constexpr std::uintptr_t swo_stimulus = 0xE0000000 + (4 * swo_port);

bool swo_enabled()
{
	return (reg(itm_tcr) & itm_tcr_itmena) != 0
		&& (reg(itm_ter) & (1U << swo_port)) != 0;
}

// A disabled port never reads FIFOREADY: the wait ends when the probe turns
// ITM off, or the target would hang once it leaves.
bool swo_write(std::uint32_t word)
{
	while ((reg(swo_stimulus) & itm_stimulus_fifoready) == 0)
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
		kymo_ring.write(bench_event_type, reg(dwt_cyccnt), bench_event{index, ~index});
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
	reg(demcr) |= demcr_trcena;   // DWT on
	reg(dwt_cyccnt) = 0;
	reg(dwt_ctrl) |= dwt_ctrl_cyccntena;

	reg(syst_rvr) = (KYMO_BENCH_CORE_CLOCK_HZ / tick_hz) - 1;
	reg(syst_cvr) = 0;
	reg(syst_csr) = syst_csr_clksource | syst_csr_tickint | syst_csr_enable;

	for (;;)
	{
	}
}

} // extern "C"
