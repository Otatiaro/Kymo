/**
 * @file    modules.cpp
 * @brief   An OpSy program on a NUCLEO-H563ZI whose tasks, handlers and
 *          mutexes live in several namespaces, as an application's functional
 *          blocks do: the timeline groups and filters them by namespace.
 *
 * The clock, the timer and the vector table are board.cpp's. Each block:
 *
 * - `app::sensors`: the timer's handler, every 4 ms, wakes `g_sampler`,
 *   which reads the bus under `drv::g_spi`;
 * - `app::sensors::imu`: `g_fusion` sleeps 10 ms, then updates its state under
 *   its own mutex;
 * - `app::comms`: the timer pends `rx_isr` every third period, which wakes
 *   `g_uplink`; it sends under `g_tx`, then logs under the global console;
 * - `app::storage`: `g_logger` writes its journal under `g_journal` and
 *   `drv::g_spi`, then pends `drv::dma_isr`, and logs under the console;
 * - `drv`: the bus's mutex and the DMA's handler, shared by the blocks above.
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

/// The console every block logs to: no namespace.
extern opsy::mutex g_console;
opsy::mutex g_console;

namespace board
{
namespace
{

std::uint32_t volatile& reg(std::uint32_t address)
{
	return *reinterpret_cast<std::uint32_t volatile*>(address);
}

constexpr std::uint32_t timer_irq = 45;  ///< TIM2
constexpr std::uint32_t comms_irq = 46;  ///< TIM3's line, pended from software; TIM3 stays off
constexpr std::uint32_t dma_irq = 47;    ///< TIM4's line, pended from software; TIM4 stays off
constexpr std::uint32_t core_clock = 8'000'000;
constexpr std::uint32_t period_cycles = core_clock / 250; ///< 4 ms

/// The timer's level, below OpSy's callable one
constexpr auto timer_priority = opsy::isr_priority::from_preempt_sub<opsy::preemption_bits>(opsy::opsy_preemption + 2, 0);

constexpr std::uint32_t rcc_cr = 0x44020C00, rcc_cfgr1 = 0x44020C1C, rcc_apb1lenr = 0x44020C9C;
constexpr std::uint32_t tim2 = 0x40000000;
constexpr std::uint32_t tim_cr1 = tim2 + 0x00, tim_dier = tim2 + 0x0C, tim_sr = tim2 + 0x10;
constexpr std::uint32_t tim_egr = tim2 + 0x14, tim_psc = tim2 + 0x28, tim_arr = tim2 + 0x2C;

constexpr std::uint32_t dwt_cyccnt = 0xE0001004;

/// Some work: `cycles` of the core, on the cycle counter, preemptions included
void spin(std::uint32_t cycles)
{
	auto const start = reg(dwt_cyccnt);
	while (reg(dwt_cyccnt) - start < cycles)
	{
	}
}

/** @brief The core on the HSE, bypassed: the ST-Link's 8 MHz */
void clock()
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
	reg(rcc_apb1lenr) |= 1u; // TIM2EN
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

} // namespace
} // namespace board

namespace drv
{
namespace
{

opsy::mutex g_spi;
std::uint32_t g_transfers = 0;

void dma_isr()
{
	++g_transfers;
}

/// A transfer on the bus, under its mutex
void transfer(std::uint32_t cycles)
{
	std::lock_guard guard{g_spi};
	board::spin(cycles);
}

} // namespace
} // namespace drv

namespace app
{
namespace
{

void log(std::uint32_t cycles)
{
	std::lock_guard guard{g_console};
	board::spin(cycles);
}

} // namespace

namespace sensors
{
namespace
{

opsy::task<1024> g_sampler;
opsy::isr_lock g_lock{board::timer_priority};
opsy::condition_variable g_tick{board::timer_priority};
std::uint32_t g_ticks = 0; ///< periods the sampler has not taken yet, under g_lock
std::uint32_t g_periods = 0;

void timer_isr()
{
	board::reg(board::tim_sr) = ~1u; // UIF
	{
		std::lock_guard guard{g_lock};
		++g_ticks;
	}
	g_tick.notify_one();
	if (++g_periods % 3 == 0)
		opsy::cortex_m::set_pending(board::comms_irq);
}

void sampler()
{
	while (true)
	{
		{
			std::lock_guard guard{g_lock};
			while (g_ticks == 0)
				g_tick.wait(g_lock);
			g_ticks = 0;
		}
		drv::transfer(board::period_cycles / 20);
	}
}

} // namespace

namespace imu
{
namespace
{

opsy::task<1024> g_fusion;
opsy::mutex g_state;

void fusion()
{
	using namespace std::chrono_literals;
	while (true)
	{
		{
			std::lock_guard guard{g_state};
			board::spin(board::period_cycles / 8);
		}
		opsy::sleep_for(10ms);
	}
}

} // namespace
} // namespace imu
} // namespace sensors

namespace comms
{
namespace
{

opsy::task<1024> g_uplink;
opsy::mutex g_tx;
opsy::isr_lock g_lock{opsy::scheduler::os_callable_priority};
opsy::condition_variable g_received{opsy::scheduler::os_callable_priority};
std::uint32_t g_frames = 0; ///< frames the uplink has not sent yet, under g_lock

void rx_isr()
{
	{
		std::lock_guard guard{g_lock};
		++g_frames;
	}
	g_received.notify_one();
}

void uplink()
{
	while (true)
	{
		{
			std::lock_guard guard{g_lock};
			while (g_frames == 0)
				g_received.wait(g_lock);
			g_frames = 0;
		}
		{
			std::lock_guard guard{g_tx};
			board::spin(board::period_cycles / 4);
		}
		log(board::period_cycles / 20);
	}
}

} // namespace
} // namespace comms

namespace storage
{
namespace
{

opsy::task<1024> g_logger;
opsy::mutex g_journal;

void logger()
{
	using namespace std::chrono_literals;
	while (true)
	{
		{
			std::lock_guard guard{g_journal};
			drv::transfer(board::period_cycles / 4);
			opsy::cortex_m::set_pending(board::dma_irq);
		}
		log(board::period_cycles / 10);
		opsy::sleep_for(14ms);
	}
}

} // namespace
} // namespace storage
} // namespace app

namespace board
{
namespace
{

// ─── The vector table: OpSy's handlers and the three interrupts ─────────────

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
	.with<timer_irq, app::sensors::timer_isr>()
	.with<comms_irq, app::comms::rx_isr>()
	.with<dma_irq, drv::dma_isr>();

constexpr std::uint32_t vtor = 0xE000ED08;
constexpr std::uint32_t demcr = 0xE000EDFC, dwt_ctrl = 0xE0001000;

opsy::idle_task<256> g_idle;

/** @brief The cycle counter that timestamps the events: on from power-up, whatever a probe set */
void start_cycle_counter()
{
	reg(demcr) |= 1u << 24; // TRCENA
	reg(dwt_ctrl) |= 1u;    // CYCCNTENA
}

/// The first task to run: priorities set from a running task, then the timer
void start()
{
	app::sensors::g_sampler.base_priority(opsy::task_priority::high);
	app::sensors::imu::g_fusion.base_priority(opsy::task_priority::normal);
	app::comms::g_uplink.base_priority(static_cast<opsy::task_priority>(0x60));
	app::storage::g_logger.base_priority(opsy::task_priority::low);
	start_timer();
	app::sensors::sampler();
}

} // namespace
} // namespace board

int opsy_test_main()
{
	board::clock();
	board::start_cycle_counter();
	board::reg(board::vtor) = reinterpret_cast<std::uint32_t>(&board::g_table);
	opsy::cortex_m::set_priority(board::timer_irq, board::timer_priority);
	opsy::cortex_m::set_priority(board::comms_irq, opsy::scheduler::os_callable_priority);
	opsy::cortex_m::set_priority(board::dma_irq, opsy::scheduler::os_callable_priority);
	opsy::cortex_m::enable_interrupt(board::timer_irq);
	opsy::cortex_m::enable_interrupt(board::comms_irq);
	opsy::cortex_m::enable_interrupt(board::dma_irq);

	(void) app::sensors::g_sampler.start([] { board::start(); });
	(void) app::sensors::imu::g_fusion.start([] { app::sensors::imu::fusion(); });
	(void) app::comms::g_uplink.start([] { app::comms::uplink(); });
	(void) app::storage::g_logger.start([] { app::storage::logger(); });
	opsy::scheduler::start(board::g_idle);
}
