/**
 * @file    fault.cpp
 * @brief   An OpSy program on a NUCLEO-H563ZI that faults in a task, freezes
 *          the trace and holds, for fault.py to record through the probe and
 *          check.
 *
 * The core runs on the HSE, bypassed: the 8 MHz the ST-Link gives on MCO.
 * `worker` samples once a millisecond, a hundred times, then reads a sensor
 * three calls deep, at an address nothing answers: a precise BusFault. The
 * adapter's hook emits kymo::fault; the application's handler then freezes
 * the trace and holds the core in place, for the probe to read the ring.
 *
 * Built without NDEBUG: the idle task spins on NOP, and the probe can read
 * the ring while nothing runs.
 */

#include "qemu_test.hpp"

#include <kymo/control.hpp>
#include <opsy.hpp>
#include <opsy_trace.hpp>

#include <chrono>
#include <cstddef>
#include <cstdint>

extern "C"
{
extern std::uint32_t _estack;
extern std::uint32_t SystemCoreClock;
void Reset_Handler();
}

namespace app
{
namespace
{

std::uint32_t volatile& reg(std::uint32_t address)
{
	return *reinterpret_cast<std::uint32_t volatile*>(address);
}

constexpr std::uint32_t core_clock = 8'000'000;
constexpr std::uint32_t rcc_cr = 0x44020C00, rcc_cfgr1 = 0x44020C1C;
constexpr std::uint32_t flash_acr = 0x40022000;
/// Where the sensor would be: an address nothing answers on the STM32H563.
constexpr std::uint32_t sensor = 0xF0000000;
constexpr std::uint32_t samples = 100;

opsy::task<1024> g_worker;
opsy::idle_task<256> g_idle;
std::uint32_t volatile g_sampled = 0;

/** @brief The core on the HSE, bypassed: the ST-Link's 8 MHz */
void board_clock()
{
	reg(rcc_cr) |= 1u << 18; // HSEBYP
	reg(rcc_cr) |= 1u << 16; // HSEON
	while ((reg(rcc_cr) & (1u << 17)) == 0) // HSERDY
	{
	}
	reg(rcc_cfgr1) = (reg(rcc_cfgr1) & ~7u) | 2u; // SW: HSE
	while (((reg(rcc_cfgr1) >> 3) & 7u) != 2u)
	{
	}
	// LATENCY and WRHIGHFREQ: no wait state, which 8 MHz allows in any voltage
	// range, where the reset value has three
	reg(flash_acr) &= ~0x3Fu;
	while ((reg(flash_acr) & 0x3Fu) != 0)
	{
	}
	SystemCoreClock = core_clock;
}

/** @brief The faulting load: the fault's pc */
[[gnu::noinline]] std::uint32_t read_sensor()
{
	return reg(sensor);
}

[[gnu::noinline]] std::uint32_t measure()
{
	auto const value = read_sensor();
	asm volatile("" ::: "memory"); // after the call: not a tail call
	return value;
}

[[gnu::noinline]] void worker()
{
	using namespace std::chrono_literals;
	for (std::uint32_t n = 0; n < samples; ++n)
	{
		g_sampled = g_sampled + 1;
		opsy::sleep_for(1ms);
	}
	g_sampled = measure();
	asm volatile("" ::: "memory");
}

/** @brief The application's handler, after the adapter's event: freezes the trace and holds */
[[noreturn]] void on_fault(const opsy::fault_status& /*status*/, opsy::exception_frame& /*frame*/, std::uint32_t /*exc_return*/)
{
	kymo::freeze();
	while (true)
	{
	}
}

// ─── The vector table: OpSy's handlers and the fault handlers ───────────────

using handler_t = void (*)();

enum class exception : std::uint8_t
{
	nmi = 2,
	hard_fault = 3,
	mem_manage = 4,
	bus_fault = 5,
	usage_fault = 6,
	svcall = 11,
	pendsv = 14,
	systick = 15
};

std::uint32_t volatile g_unexpected = 0; ///< the exception the table did not expect, for a debugger

[[noreturn]] void unexpected_exception()
{
	g_unexpected = opsy::cortex_m::ipsr();
	while (true)
	{
	}
}

/** @brief The shape SvdGenerator generates, enough of it for with() */
struct alignas(512) vector_table
{
	using exception_type = exception;

	constexpr vector_table(std::uint32_t* stack, handler_t reset) : initial_sp(stack)
	{
		for (auto& handler : handlers)
			handler = &unexpected_exception;
		handlers[0] = reset;
	}

	template<exception Exception, handler_t Handler>
	constexpr auto with() const -> vector_table
	{
		auto copy = *this;
		copy.handlers[static_cast<std::size_t>(Exception) - 1] = Handler;
		return copy;
	}

	constexpr auto with(auto&& install) const { return install(*this); }

	std::uint32_t* initial_sp;
	handler_t handlers[15 + 64];
};

constexpr auto g_table = vector_table(&_estack, Reset_Handler)
	.with(opsy::scheduler::isrs)
	.with(opsy::fault_isrs<on_fault>);

constexpr std::uint32_t vtor = 0xE000ED08;
constexpr std::uint32_t demcr = 0xE000EDFC, dwt_ctrl = 0xE0001000;

/** @brief The cycle counter that timestamps the events: on from power-up, whatever a probe set */
void start_cycle_counter()
{
	reg(demcr) |= 1u << 24; // TRCENA
	reg(dwt_ctrl) |= 1u;    // CYCCNTENA
}

} // namespace
} // namespace app

int opsy_test_main()
{
	app::board_clock();
	app::start_cycle_counter();
	app::reg(app::vtor) = reinterpret_cast<std::uint32_t>(&app::g_table);
	opsy::cortex_m::enable_configurable_faults();

	(void) app::g_worker.start([] { app::worker(); });
	opsy::scheduler::start(app::g_idle);
}
