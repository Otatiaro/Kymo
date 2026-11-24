/**
 * @file    board.cpp
 * @brief   An OpSy program on a NUCLEO-H563ZI, traced by the adapter, for
 *          board.py to record through the probe and check.
 *
 * The core runs on the HSE, bypassed: the 8 MHz the ST-Link gives on MCO.
 * TIM2 counts that clock and interrupts once a millisecond. Its handler wakes
 * `sampler` through a condition variable, then pends a second interrupt, of a
 * higher priority, that preempts it on the spot: one nested interrupt per
 * period.
 *
 * Two tasks share a mutex. `logger`, the lower, holds it for about half a
 * period, then sleeps; `sampler`, woken by the timer, takes it. When the
 * timer comes while `logger` holds it, `sampler` blocks, `logger` inherits
 * its priority, and the mutex is handed over on release.
 *
 * Built without NDEBUG: the idle task spins on NOP, and the probe can read
 * the ring while nothing runs.
 */

#include "qemu_test.hpp"

#include <opsy.hpp>

#include <cstddef>
#include <cstdint>
#include <mutex>

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

constexpr std::uint32_t timer_irq = 45;  ///< TIM2
constexpr std::uint32_t nested_irq = 46; ///< TIM3's line, pended from software; TIM3 stays off
constexpr std::uint32_t core_clock = 8'000'000;
constexpr std::uint32_t period_cycles = core_clock / 1000;

/// The timer's level, below OpSy's callable one, at which the nested interrupt sits
constexpr auto timer_priority = opsy::isr_priority::from_preempt_sub<opsy::preemption_bits>(opsy::opsy_preemption + 2, 0);

constexpr std::uint32_t rcc_cr = 0x44020C00, rcc_cfgr1 = 0x44020C1C, rcc_apb1lenr = 0x44020C9C;
constexpr std::uint32_t tim2 = 0x40000000;
constexpr std::uint32_t tim_cr1 = tim2 + 0x00, tim_dier = tim2 + 0x0C, tim_sr = tim2 + 0x10;
constexpr std::uint32_t tim_egr = tim2 + 0x14, tim_psc = tim2 + 0x28, tim_arr = tim2 + 0x2C;

opsy::task<1024> g_sampler;
opsy::task<1024> g_logger;
opsy::idle_task<256> g_idle;

opsy::mutex g_mutex;
opsy::isr_lock g_lock{timer_priority};
opsy::condition_variable g_tick{timer_priority};
std::uint32_t g_ticks = 0;  ///< periods sampler has not taken yet, under g_lock
std::uint32_t g_nested = 0; ///< nested interrupts taken
std::uint32_t g_shared = 0; ///< under g_mutex

void timer_isr()
{
	reg(tim_sr) = ~1u; // UIF
	{
		std::lock_guard guard{g_lock};
		++g_ticks;
	}
	g_tick.notify_one();
	opsy::cortex_m::set_pending(nested_irq); // taken here, before this handler returns
}

void nested_isr()
{
	++g_nested;
}

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
	SystemCoreClock = core_clock;
}

/** @brief TIM2 on the core clock, an update interrupt every period */
void start_timer()
{
	reg(rcc_apb1lenr) |= 1u; // TIM2EN, which the timer needs two cycles to see
	while ((reg(rcc_apb1lenr) & 1u) == 0)
	{
	}
	reg(tim_psc) = 0;
	reg(tim_arr) = period_cycles - 1;
	reg(tim_egr) = 1u; // UG: loads the prescaler
	reg(tim_sr) = 0;
	reg(tim_dier) = 1u; // UIE
	reg(tim_cr1) = 1u;  // CEN
}

void sampler()
{
	// From a running task: a priority change before scheduler::start asks
	// for a switch with no process stack to save to.
	g_sampler.base_priority(opsy::task_priority::high);
	g_logger.base_priority(opsy::task_priority::low);
	start_timer();
	while (true)
	{
		{
			std::lock_guard guard{g_lock};
			while (g_ticks == 0)
				g_tick.wait(g_lock);
			g_ticks = 0;
		}
		std::lock_guard guard{g_mutex};
		++g_shared;
	}
}

void logger()
{
	using namespace std::chrono_literals;
	while (true)
	{
		{
			std::lock_guard guard{g_mutex};
			for (std::uint32_t volatile k = 0; k < period_cycles / 16; k = k + 1)
			{
			}
		}
		opsy::sleep_for(2ms);
	}
}

// ─── The vector table: OpSy's handlers and the two interrupts ───────────────

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

	template<std::uint32_t Irq, handler_t Handler>
	constexpr auto with() const -> vector_table
	{
		auto copy = *this;
		copy.handlers[15 + Irq] = opsy::hooked_isr::wrap<Handler>;
		return copy;
	}

	constexpr auto with(auto&& install) const { return install(*this); }

	std::uint32_t* initial_sp;
	handler_t handlers[15 + 64];
};

constexpr auto g_table = vector_table(&_estack, Reset_Handler)
	.with(opsy::scheduler::isrs)
	.with<timer_irq, timer_isr>()
	.with<nested_irq, nested_isr>();

constexpr std::uint32_t vtor = 0xE000ED08;

} // namespace
} // namespace app

int opsy_test_main()
{
	app::board_clock();
	app::reg(app::vtor) = reinterpret_cast<std::uint32_t>(&app::g_table);
	opsy::cortex_m::set_priority(app::timer_irq, app::timer_priority);
	opsy::cortex_m::set_priority(app::nested_irq, opsy::scheduler::os_callable_priority);
	opsy::cortex_m::enable_interrupt(app::timer_irq);
	opsy::cortex_m::enable_interrupt(app::nested_irq);

	(void) app::g_sampler.start([] { app::sampler(); });
	(void) app::g_logger.start([] { app::logger(); });
	opsy::scheduler::start(app::g_idle);
}
