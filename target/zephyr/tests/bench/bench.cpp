/**
 * @file    bench.cpp
 * @brief   The paths through Zephyr the adapter adds to, each between two
 *          markers, for what the trace costs on each.
 *
 * The image is built twice, untraced (prj.conf) and traced (with
 * traced.conf), and target/opsy/tests/bench/overhead.py, through OpSy's
 * tests/qemu/bench/profile.py, counts the instructions QEMU executes between
 * bench::begin<S>() and bench::end<S>() in each: the difference is the
 * trace's cost on that path. The markers are one pair of functions per
 * scenario, so the trace tells them apart by address. Each scenario runs
 * bench::iterations times; profile.py keeps the median.
 *
 * Two threads: main, the driver, the higher, runs the scenarios and is the
 * side that gets woken; `other`, the lower, is the side that wakes it,
 * directly or by pending the interrupt whose handler does. Nothing else runs:
 * SysTick, and the idle thread when both wait.
 *
 * The scenarios are FreeRTOS's bench's (target/freertos/tests/bench), under
 * the same names, less the task notification, which Zephyr has not.
 *
 * Built for a board (KYMO_BENCH_BOARD), the markers read the DWT cycle
 * counter and the image prints each scenario's minimum, median and maximum
 * in cycles over semihosting, as OpSy's and FreeRTOS's bench images do.
 */
#include "qemu.hpp"

#include <zephyr/irq.h>
#include <zephyr/kernel.h>

#include <cmsis_core.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>

namespace bench
{

enum class scenario : std::uint8_t
{
	switch_ping_pong,   ///< driver gives to other and takes back: two switches
	semaphore_from_isr, ///< driver in k_sem_take, the handler gives
	queue_from_isr,     ///< driver in k_msgq_get, the handler puts
	mutex_free,         ///< a mutex locked and unlocked with nobody else
	mutex_contended,    ///< other holds it, driver blocks and other inherits, other unlocks
	sleep_one_tick,     ///< from the SysTick that ends a k_sleep of a tick to its return
	release_no_switch,  ///< driver gives what the lower other waits on: nobody switches
	count,
};

constexpr std::array<char const*, static_cast<std::size_t>(scenario::count)> names{
	"switch_ping_pong", "semaphore_from_isr", "queue_from_isr", "mutex_free",
	"mutex_contended", "sleep_one_tick", "release_no_switch",
};

constexpr std::size_t iterations = 8;
constexpr std::size_t stack_size = 1024;
constexpr int other_priority = 5;
/// The interrupt the handler serves: the last of the part, which no driver
/// uses.
constexpr unsigned device_irq = CONFIG_NUM_IRQS - 1;

/// The scenario between its markers: the store also keeps the markers'
/// bodies apart, so no two are merged into one address.
scenario volatile measuring = scenario::count;

#ifdef KYMO_BENCH_BOARD
/// Cycles per scenario and iteration.
std::array<std::array<std::uint32_t, iterations>, static_cast<std::size_t>(scenario::count)> results{};
std::uint32_t begun = 0;
std::size_t iteration = 0;
#endif

template<scenario S>
[[gnu::noinline]] void begin()
{
	measuring = S;
#ifdef KYMO_BENCH_BOARD
	begun = DWT->CYCCNT;
#endif
}

template<scenario S>
[[gnu::noinline]] void end()
{
#ifdef KYMO_BENCH_BOARD
	// NOLINTNEXTLINE(cppcoreguidelines-pro-bounds-constant-array-index): iteration < iterations, run's loop
	results[static_cast<std::size_t>(S)][iteration] = DWT->CYCCNT - begun;
#endif
	measuring = scenario::count;
}

K_SEM_DEFINE(other_go, 0, 1);    ///< driver → other: your turn
K_SEM_DEFINE(driver_go, 0, 1);   ///< other → driver: done
K_SEM_DEFINE(semaphore, 0, 1);
K_MUTEX_DEFINE(mutex);
// NOLINTNEXTLINE(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays,cppcoreguidelines-pro-bounds-array-to-pointer-decay,cppcoreguidelines-pro-bounds-pointer-arithmetic,misc-use-anonymous-namespace): the kernel's definition macro
K_MSGQ_DEFINE(queue, sizeof(std::uint32_t), 1, 4);

/// The scenario in progress, for the handlers and for other.
scenario volatile pending = scenario::count;

/** @brief Connected through the kernel's wrapper, as an application's is. */
void device_handler(void const* /*unused*/)
{
	switch (pending)
	{
	case scenario::semaphore_from_isr:
		begin<scenario::semaphore_from_isr>();
		k_sem_give(&semaphore);
		break;
	case scenario::queue_from_isr:
	{
		begin<scenario::queue_from_isr>();
		std::uint32_t const value = 1;
		k_msgq_put(&queue, &value, K_NO_WAIT);
		break;
	}
	default:
		break;
	}
}

/// Each turn driver gives it is one step of the scenario in progress.
void other(void* /*unused*/, void* /*unused*/, void* /*unused*/)
{
	for (;;)
	{
		k_sem_take(&other_go, K_FOREVER);
		switch (pending)
		{
		case scenario::switch_ping_pong:
		case scenario::release_no_switch:
			k_sem_give(&driver_go);
			break;
		case scenario::mutex_contended:
			k_mutex_lock(&mutex, K_FOREVER);
			k_sem_give(&driver_go);   // driver preempts, blocks on the mutex and lends us its priority
			k_mutex_unlock(&mutex);   // and runs again here
			break;
		default:
			NVIC_SetPendingIRQ(static_cast<IRQn_Type>(device_irq));   // NOLINT(clang-analyzer-optin.core.EnumCastOutOfRange): an interrupt the part's enum does not name
			__DSB();
			__ISB();
			break;
		}
	}
}

template<scenario S, typename Once>
void run(Once once)
{
	pending = S;
	for (std::size_t i = 0; i < iterations; ++i)
	{
#ifdef KYMO_BENCH_BOARD
		iteration = i;
#endif
		once();
	}
	pending = scenario::count;
}

void driver()
{
	run<scenario::switch_ping_pong>([] {
		begin<scenario::switch_ping_pong>();
		k_sem_give(&other_go);
		k_sem_take(&driver_go, K_FOREVER);
		end<scenario::switch_ping_pong>();
	});

	run<scenario::semaphore_from_isr>([] {
		k_sem_give(&other_go);
		k_sem_take(&semaphore, K_FOREVER);
		end<scenario::semaphore_from_isr>();
	});

	run<scenario::queue_from_isr>([] {
		k_sem_give(&other_go);
		std::uint32_t value = 0;
		k_msgq_get(&queue, &value, K_FOREVER);
		end<scenario::queue_from_isr>();
	});

	run<scenario::mutex_free>([] {
		begin<scenario::mutex_free>();
		k_mutex_lock(&mutex, K_FOREVER);
		k_mutex_unlock(&mutex);
		end<scenario::mutex_free>();
	});

	run<scenario::mutex_contended>([] {
		k_sem_give(&other_go);
		k_sem_take(&driver_go, K_FOREVER);   // other holds the mutex now
		begin<scenario::mutex_contended>();
		k_mutex_lock(&mutex, K_FOREVER);     // blocks, other inherits, other unlocks, we run
		end<scenario::mutex_contended>();
		k_mutex_unlock(&mutex);
	});

	run<scenario::sleep_one_tick>([] {
		k_sleep(K_TICKS(1));   // begin is the SysTick that ends it: see __wrap_sys_clock_isr()
		end<scenario::sleep_one_tick>();
	});

	run<scenario::release_no_switch>([] {
		begin<scenario::release_no_switch>();
		k_sem_give(&other_go);   // other becomes ready; it is lower, nobody switches
		end<scenario::release_no_switch>();
		k_sem_take(&driver_go, K_FOREVER);   // let it take its turn
		k_sleep(K_TICKS(1));                 // and get back into its take: we preempted it inside its give
	});

#ifdef KYMO_BENCH_BOARD
	for (std::size_t s = 0; s < names.size(); ++s)
	{
		auto& samples = results[s];   // NOLINT(cppcoreguidelines-pro-bounds-constant-array-index): s < names.size()
		std::sort(samples.begin(), samples.end());
		qemu::write("BENCH ");
		qemu::write(names[s]);        // NOLINT(cppcoreguidelines-pro-bounds-constant-array-index): s < names.size()
		qemu::write(" min=");
		qemu::write_unsigned(samples.front());
		qemu::write(" median=");
		qemu::write_unsigned(samples[iterations / 2]);
		qemu::write(" max=");
		qemu::write_unsigned(samples.back());
		qemu::write("\n");
	}
#else
	// profile.py takes the scenarios' names from these lines, in enum order.
	for (auto const* name : names)
	{
		qemu::write("BENCH ");
		qemu::write(name);
		qemu::write(" done\n");
	}
#endif
	qemu::finish(true);
}

} // namespace bench

extern "C"
{
void __real_sys_clock_isr();   // NOLINT(bugprone-reserved-identifier,cert-dcl37-c,cert-dcl51-cpp): the linker's name for the wrapped handler

/** @brief The kernel's SysTick handler, wrapped by the link (CMakeLists.txt):
 *  every tick during the sleep scenario begins it again, and the last one
 *  before driver runs is the one that woke it. */
void __wrap_sys_clock_isr()   // NOLINT(bugprone-reserved-identifier,cert-dcl37-c,cert-dcl51-cpp): the linker's name for the wrapper
{
	if (bench::pending == bench::scenario::sleep_one_tick)
		bench::begin<bench::scenario::sleep_one_tick>();
	__real_sys_clock_isr();
}
}

// NOLINTBEGIN(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays,cppcoreguidelines-pro-bounds-array-to-pointer-decay,readability-math-missing-parentheses,bugprone-branch-clone,misc-misplaced-const): the kernel's definition macro
K_THREAD_DEFINE(other, bench::stack_size, bench::other, nullptr, nullptr, nullptr, bench::other_priority, 0, 0);
// NOLINTEND(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays,cppcoreguidelines-pro-bounds-array-to-pointer-decay,readability-math-missing-parentheses,bugprone-branch-clone,misc-misplaced-const)

int main()
{
	using bench::device_handler;   // the kernel's macro pastes the name into a symbol's
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-cstyle-cast,misc-redundant-expression,bugprone-signed-bitwise,misc-const-correctness): the kernel's macro
	IRQ_CONNECT(bench::device_irq, 1, device_handler, nullptr, 0);
	irq_enable(bench::device_irq);
#ifdef KYMO_BENCH_BOARD
	CoreDebug->DEMCR |= CoreDebug_DEMCR_TRCENA_Msk;
	DWT->CYCCNT = 0;
	DWT->CTRL |= DWT_CTRL_CYCCNTENA_Msk;
#endif
	bench::driver();
	return 0;
}
