/**
 * @file    bench.cpp
 * @brief   The paths through FreeRTOS the adapter adds to, each between two
 *          markers, for what the trace costs on each.
 *
 * The image is built twice, untraced (KYMO_UNTRACED) and traced, and
 * bench/overhead.py, through OpSy's tests/qemu/bench/profile.py, counts the
 * instructions QEMU executes between bench::begin<S>() and bench::end<S>()
 * in each: the difference is the trace's cost on that path. The markers are
 * one pair of functions per scenario, so the trace tells them apart by
 * address. Each scenario runs bench::iterations times; profile.py keeps the
 * median.
 *
 * Two tasks: `driver`, the higher one, runs the scenarios and is the side
 * that gets woken; `other`, the lower one, is the side that wakes it,
 * directly or by pending the interrupt whose handler does. Nothing else
 * runs: SysTick, and the idle task when both wait.
 *
 * Built with KYMO_BENCH_BOARD for a NUCLEO-H563ZI: the clock goes to 250 MHz,
 * the markers read the DWT cycle counter, and the image prints each
 * scenario's minimum, median and maximum in cycles over semihosting, as
 * OpSy's bench image does on the same board.
 */
#include "platform.hpp"
#include "qemu.hpp"

#include <FreeRTOS.h>
#include <semphr.h>
#include <task.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>

namespace bench
{

enum class scenario : std::uint8_t
{
	switch_ping_pong,   ///< driver gives to other and takes back: two switches
	semaphore_from_isr, ///< driver in xSemaphoreTake, the handler gives
	notify_from_isr,    ///< driver in ulTaskNotifyTake, the handler notifies
	queue_from_isr,     ///< driver in xQueueReceive, the handler sends
	mutex_free,         ///< a mutex taken and given with nobody else
	mutex_contended,    ///< other holds it, driver blocks and other inherits, other gives
	sleep_one_tick,     ///< from the SysTick that ends a vTaskDelay(1) to its return
	release_no_switch,  ///< driver gives what the lower other waits on: nobody switches
	count,
};

constexpr std::array<char const*, static_cast<std::size_t>(scenario::count)> names{
	"switch_ping_pong", "semaphore_from_isr", "notify_from_isr", "queue_from_isr",
	"mutex_free", "mutex_contended", "sleep_one_tick", "release_no_switch",
};

constexpr std::size_t iterations = 8;

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
	begun = platform::cycle_count();
#endif
}

template<scenario S>
[[gnu::noinline]] void end()
{
#ifdef KYMO_BENCH_BOARD
	// NOLINTNEXTLINE(cppcoreguidelines-pro-bounds-constant-array-index): iteration < iterations, run's loop
	results[static_cast<std::size_t>(S)][iteration] = platform::cycle_count() - begun;
#endif
	measuring = scenario::count;
}

constexpr std::size_t stack_depth = configMINIMAL_STACK_SIZE;

StaticTask_t driver_tcb, other_tcb;
std::array<StackType_t, stack_depth> driver_stack, other_stack;
TaskHandle_t driver_task;

StaticSemaphore_t other_go_storage, driver_go_storage, semaphore_storage, mutex_storage;
SemaphoreHandle_t other_go;   ///< driver → other: your turn
SemaphoreHandle_t driver_go;  ///< other → driver: done
SemaphoreHandle_t semaphore, mutex;

StaticQueue_t queue_storage;
std::array<std::uint8_t, sizeof(std::uint32_t)> queue_items;
QueueHandle_t queue;

/// The scenario in progress, for the handlers and for other.
scenario volatile pending = scenario::count;

/// The port's SysTick, after the sleep scenario's begin: every tick during
/// the scenario begins again, and the last one before driver runs is the one
/// that woke it.
void systick()
{
	if (pending == scenario::sleep_one_tick)
		begin<scenario::sleep_one_tick>();
	platform::port_systick();
}

void device_handler()
{
	traceISR_ENTER();   // NOLINT(misc-include-cleaner): the adapter's, or FreeRTOS.h's empty default untraced
	BaseType_t woken = pdFALSE;
	switch (pending)
	{
	case scenario::semaphore_from_isr:
		begin<scenario::semaphore_from_isr>();
		xSemaphoreGiveFromISR(semaphore, &woken);
		break;
	case scenario::notify_from_isr:
		begin<scenario::notify_from_isr>();
		vTaskNotifyGiveFromISR(driver_task, &woken);
		break;
	case scenario::queue_from_isr:
	{
		begin<scenario::queue_from_isr>();
		std::uint32_t const value = 1;
		xQueueSendFromISR(queue, &value, &woken);
		break;
	}
	default:
		break;
	}
	portYIELD_FROM_ISR(woken);   // NOLINT(cppcoreguidelines-pro-type-cstyle-cast): the port's macro
}

/// Each turn driver gives it is one step of the scenario in progress.
void other(void* /*unused*/)
{
	for (;;)
	{
		xSemaphoreTake(other_go, portMAX_DELAY);
		switch (pending)
		{
		case scenario::switch_ping_pong:
		case scenario::release_no_switch:
			xSemaphoreGive(driver_go);
			break;
		case scenario::mutex_contended:
			xSemaphoreTake(mutex, portMAX_DELAY);
			xSemaphoreGive(driver_go);   // driver preempts, blocks on the mutex and lends us its priority
			xSemaphoreGive(mutex);       // and runs again here
			break;
		default:
			platform::pend();   // the handler does the rest
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

void driver(void* /*unused*/)
{
	run<scenario::switch_ping_pong>([] {
		begin<scenario::switch_ping_pong>();
		xSemaphoreGive(other_go);
		xSemaphoreTake(driver_go, portMAX_DELAY);
		end<scenario::switch_ping_pong>();
	});

	run<scenario::semaphore_from_isr>([] {
		xSemaphoreGive(other_go);
		xSemaphoreTake(semaphore, portMAX_DELAY);
		end<scenario::semaphore_from_isr>();
	});

	run<scenario::notify_from_isr>([] {
		xSemaphoreGive(other_go);
		ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
		end<scenario::notify_from_isr>();
	});

	run<scenario::queue_from_isr>([] {
		xSemaphoreGive(other_go);
		std::uint32_t value = 0;
		xQueueReceive(queue, &value, portMAX_DELAY);
		end<scenario::queue_from_isr>();
	});

	run<scenario::mutex_free>([] {
		begin<scenario::mutex_free>();
		xSemaphoreTake(mutex, portMAX_DELAY);
		xSemaphoreGive(mutex);
		end<scenario::mutex_free>();
	});

	run<scenario::mutex_contended>([] {
		xSemaphoreGive(other_go);
		xSemaphoreTake(driver_go, portMAX_DELAY);   // other holds the mutex now
		begin<scenario::mutex_contended>();
		xSemaphoreTake(mutex, portMAX_DELAY);       // blocks, other inherits, other gives, we run
		end<scenario::mutex_contended>();
		xSemaphoreGive(mutex);
	});

	run<scenario::sleep_one_tick>([] {
		vTaskDelay(1);   // begin is the SysTick that ends it: see systick()
		end<scenario::sleep_one_tick>();
	});

	run<scenario::release_no_switch>([] {
		begin<scenario::release_no_switch>();
		xSemaphoreGive(other_go);   // other becomes ready; it is lower, nobody switches
		end<scenario::release_no_switch>();
		xSemaphoreTake(driver_go, portMAX_DELAY);   // let it take its turn
		vTaskDelay(1);   // and get back into its take: we preempted it inside its give
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

extern "C" void application_assert(char const* file, int line)
{
	qemu::write("assertion failed: ");
	qemu::write(file);
	qemu::write(":");
	qemu::write_unsigned(static_cast<std::uint32_t>(line));
	qemu::write("\n");
	qemu::finish(false);
}

void test_main()
{
	using namespace bench;
#ifdef KYMO_BENCH_BOARD
	platform::clock_250mhz();
	platform::start_cycle_count();
#endif
	platform::prepare(&systick, &device_handler);

	other_go = xSemaphoreCreateBinaryStatic(&other_go_storage);
	driver_go = xSemaphoreCreateBinaryStatic(&driver_go_storage);
	semaphore = xSemaphoreCreateBinaryStatic(&semaphore_storage);
	mutex = xSemaphoreCreateMutexStatic(&mutex_storage);
	queue = xQueueCreateStatic(1, sizeof(std::uint32_t), queue_items.data(), &queue_storage);

	driver_task = xTaskCreateStatic(driver, "driver", driver_stack.size(), nullptr, 3, driver_stack.data(), &driver_tcb);
	xTaskCreateStatic(other, "other", other_stack.size(), nullptr, 1, other_stack.data(), &other_tcb);

	vTaskStartScheduler();
	application_assert(__FILE__, __LINE__);   // the scheduler does not return
}
