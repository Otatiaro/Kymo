/**
 * @file    controller.cpp
 * @brief   An OpSy program on a NUCLEO-H563ZI whose alarm comes now and then,
 *          for controller.py to record through the probe and check why.
 *
 * The core runs on the HSE, bypassed: the 8 MHz the ST-Link gives on MCO.
 * TIM2 interrupts once a millisecond: its handler wakes `controller`, which
 * runs a cycle, a duration `cycle`, and marks it done. When the next tick
 * finds the cycle not done, the handler emits an `alarm`.
 *
 * A cycle ends about three quarters of a period after its tick, the tick's
 * handler and the switch to the controller included. TIM3 interrupts every 50
 * periods and 368 cycles: its handler, the link's, copies a burst for about
 * 375 µs. Its phase in the period moves by 368 cycles each time: a run of
 * bursts lands in the cycle, preempts it past the next tick, and raises the
 * alarm; the bursts between land while the core is idle or in `logger`, and
 * nothing happens.
 *
 * `logger`, the lower task, works for about a quarter of a period, then
 * sleeps for 3 ms.
 *
 * Built without NDEBUG: the idle task spins on NOP, and the probe can read
 * the ring while nothing runs.
 */

#include "qemu_test.hpp"

#include <opsy.hpp>
#include <opsy_trace.hpp>

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

constexpr std::uint32_t timer_irq = 45; ///< TIM2
constexpr std::uint32_t link_irq = 46;  ///< TIM3
constexpr std::uint32_t core_clock = 8'000'000;
constexpr std::uint32_t period_cycles = core_clock / 1000;

constexpr std::uint32_t cycle_work = 3600;  ///< cycles of a cycle's own work
constexpr std::uint32_t burst_work = 3000;  ///< cycles the link's handler takes
constexpr std::uint32_t logger_work = 2000; ///< cycles of the logger's work
/// TIM3 counts every 8 cycles: 50046 counts are 50 periods and 368 cycles.
constexpr std::uint32_t link_prescaler = 8;
constexpr std::uint32_t link_counts = 50046;

constexpr auto irq_priority = opsy::isr_priority::from_preempt_sub<opsy::preemption_bits>(opsy::opsy_preemption + 2, 0);

constexpr std::uint32_t rcc_cr = 0x44020C00, rcc_cfgr1 = 0x44020C1C, rcc_apb1lenr = 0x44020C9C;
constexpr std::uint32_t tim2 = 0x40000000, tim3 = 0x40000400;
constexpr std::uint32_t tim_cr1 = 0x00, tim_dier = 0x0C, tim_sr = 0x10, tim_egr = 0x14, tim_psc = 0x28, tim_arr = 0x2C;

opsy::task<1024> g_controller;
opsy::task<1024> g_logger;
opsy::idle_task<256> g_idle;

opsy::isr_lock g_lock{irq_priority};
opsy::condition_variable g_tick{irq_priority};
std::uint32_t g_ticks = 0;     ///< periods the controller has not taken yet, under g_lock
bool volatile g_done = true;   ///< the last cycle is done
std::uint32_t g_alarms = 0;    ///< alarms raised
std::uint32_t g_cycle = 0;     ///< cycles run
std::uint32_t g_bursts = 0;    ///< bursts copied

/// Something the controller expected was not ready on the tick.
struct alarm
{
	std::uint16_t code;
	std::uint32_t count;
};

/// A cycle of the controller: from its wake to its outputs ready.
struct cycle
{
	std::uint32_t number;
};

/// Runs for @p cycles of the core, on the cycle counter.
void spin(std::uint32_t cycles)
{
	std::uint32_t const start = kymo::timestamp();
	while (kymo::timestamp() - start < cycles)
	{
	}
}

void timer_isr()
{
	reg(tim2 + tim_sr) = ~1u; // UIF
	if (!g_done)
	{
		++g_alarms;
		trace::emit(alarm{0x2C, g_alarms});
	}
	g_done = false;
	{
		std::lock_guard guard{g_lock};
		++g_ticks;
	}
	g_tick.notify_one();
}

void link_isr()
{
	reg(tim3 + tim_sr) = ~1u; // UIF
	++g_bursts;
	spin(burst_work);
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

/** @brief A timer of the APB1 bus counting the core clock divided by @p prescaler, an update interrupt every @p counts */
void start_timer(std::uint32_t timer, std::uint32_t enable_bit, std::uint32_t prescaler, std::uint32_t counts)
{
	reg(rcc_apb1lenr) |= enable_bit; // which the timer needs two cycles to see
	while ((reg(rcc_apb1lenr) & enable_bit) == 0)
	{
	}
	reg(timer + tim_psc) = prescaler - 1;
	reg(timer + tim_arr) = counts - 1;
	reg(timer + tim_egr) = 1u; // UG: loads the prescaler
	reg(timer + tim_sr) = 0;
	reg(timer + tim_dier) = 1u; // UIE
	reg(timer + tim_cr1) = 1u;  // CEN
}

void controller()
{
	// From a running task: a priority change before scheduler::start asks
	// for a switch with no process stack to save to.
	g_controller.base_priority(opsy::task_priority::high);
	g_logger.base_priority(opsy::task_priority::low);
	start_timer(tim2, 1u << 0, 1, period_cycles);
	start_timer(tim3, 1u << 1, link_prescaler, link_counts);
	while (true)
	{
		{
			std::lock_guard guard{g_lock};
			while (g_ticks == 0)
				g_tick.wait(g_lock);
			g_ticks = 0;
		}
		auto const running = trace::scope(cycle{g_cycle});
		spin(cycle_work);
		++g_cycle;
		g_done = true;
	}
}

void logger()
{
	using namespace std::chrono_literals;
	while (true)
	{
		spin(logger_work);
		opsy::sleep_for(3ms);
	}
}

// ─── The vector table: OpSy's handlers and the two timers ────────────────────

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
	.with<link_irq, link_isr>();

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
	opsy::cortex_m::set_priority(app::timer_irq, app::irq_priority);
	opsy::cortex_m::set_priority(app::link_irq, app::irq_priority);
	opsy::cortex_m::enable_interrupt(app::timer_irq);
	opsy::cortex_m::enable_interrupt(app::link_irq);

	(void) app::g_controller.start([] { app::controller(); });
	(void) app::g_logger.start([] { app::logger(); });
	opsy::scheduler::start(app::g_idle);
}
