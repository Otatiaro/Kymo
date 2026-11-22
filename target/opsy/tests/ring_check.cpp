/**
 * @file    ring_check.cpp
 * @brief   What the adapter writes, read back from the ring: a case of
 *          OpSy's on-target suite, linked into its image.
 *
 * An interrupt handler, decorated as a vector table decorates it, wakes a
 * task waiting on a condition variable, twice. With the default control word
 * the ring must then hold, in this order: the handler's entry, the
 * notification, the task made ready, the handler's exit, the task started;
 * OpSy's own handlers are family kernel, off by default, and none of their
 * events is written. With kernel enabled, PendSV comes in before the task
 * starts. SysTick may come in between and is skipped. Each event carries the
 * address of its handler, condition variable or task.
 */

#include "qemu_test.hpp"

#include <opsy.hpp>

#include <kymo/opsy/hooks.hpp>

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <mutex>

using namespace std::chrono_literals;

static_assert(KYMO_CONTROL_DEFAULT == ~kymo::bit(kymo::family::kernel), "kernel alone is off by default");

namespace
{

opsy::task<1024> g_worker;
opsy::isr_lock g_lock{opsy::scheduler::os_callable_priority};
opsy::condition_variable g_wake{opsy::scheduler::os_callable_priority};
bool g_raised = false;
std::atomic<int> g_woken = 0;
unsigned g_irq = qemu_test::no_irq;

void wake_worker()
{
	{
		std::lock_guard guard{g_lock};
		g_raised = true;
	}
	g_wake.notify_one();
}

struct read_event
{
	std::uint32_t type;
	std::uint32_t first_word; ///< the first payload word
};

/**
 * @brief The events written from @p from on, first slots only, SysTick's
 *        left out. A slot whose lap is not its index's is the start of an
 *        event that ran into the guard zone: its slots are past the end.
 */
std::size_t read_events(std::uint32_t from, read_event* out, std::size_t capacity)
{
	auto& ring = kymo::trace_ring;
	using ring_t = std::remove_reference_t<decltype(ring)>;
	constexpr std::uint32_t mask = KYMO_RING_SLOTS - 1;
	std::uint32_t const systick_in = kymo::type_id<kymo::opsy::enter_systick>();
	std::uint32_t const systick_out = kymo::type_id<kymo::opsy::exit_systick>();

	std::size_t count = 0;
	std::uint32_t const to = ring.write_index.load(std::memory_order_acquire);
	for (std::uint32_t index = from; index != to && count < capacity; ++index)
	{
		auto const& slot = ring.slots[index & mask];
		std::uint32_t const header = slot.header.load(std::memory_order_acquire);
		if ((header & kymo::head_bit) == 0 || header != ring_t::header(header & 0xffffu, index, true))
			continue;
		std::uint32_t const type = header & 0xffffu;
		if (type == systick_in || type == systick_out)
			continue;
		out[count++] = {type, slot.words[1]};
	}
	return count;
}

template<typename Event>
std::uint32_t id()
{
	return kymo::type_id<Event>();
}

std::uint32_t address(void const* p)
{
	return static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(p));
}

std::uint32_t address(void (*handler)())
{
	return static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(handler));
}

/// Raises the interrupt once, and reads back what it caused.
std::size_t wake_once(read_event* events, std::size_t capacity)
{
	int const woken = g_woken;
	std::uint32_t const from = kymo::trace_ring.write_index.load(std::memory_order_relaxed);
	qemu_test::raise(g_irq);
	CHECK(g_woken == woken + 1);
	return read_events(from, events, capacity);
}

OPSY_QEMU_TEST(the_adapter_records_an_interrupt_waking_a_task)
{
	g_raised = false;
	g_woken = 0;
	CHECK(g_worker.start([] {
		while (true)
		{
			{
				std::lock_guard guard{g_lock};
				while (!g_raised)
					g_wake.wait(g_lock);
				g_raised = false;
			}
			++g_woken;
		}
	}));
	g_worker.priority(opsy::task_priority::highest);
	opsy::sleep_for(2ms);

	if (g_irq == qemu_test::no_irq)
		g_irq = qemu_test::find_usable_irq();
	CHECK(g_irq != qemu_test::no_irq);
	opsy::cortex_m::set_priority(g_irq, opsy::scheduler::os_callable_priority);
	opsy::cortex_m::set_isr_handler(g_irq, opsy::hooked_isr::wrap<&wake_worker>);
	opsy::cortex_m::enable_interrupt(g_irq);

	opsy::task_control_block const& worker = g_worker;
	read_event const isr_in{id<kymo::opsy::isr_enter>(), address(&wake_worker)};
	read_event const notified{id<kymo::opsy::condition_variable_notify_one>(), address(&g_wake)};
	read_event const ready{id<kymo::opsy::task_ready>(), address(&worker)};
	read_event const isr_out{id<kymo::opsy::isr_exit>(), address(&wake_worker)};
	read_event const started{id<kymo::opsy::task_started>(), address(&worker)};

	// The default control word: nothing of OpSy's own handlers.
	read_event events[16] = {};
	std::size_t count = wake_once(events, 16);
	CHECK(count >= 5);
	read_event const by_default[] = {isr_in, notified, ready, isr_out, started};
	for (std::size_t i = 0; i < 5; ++i)
	{
		CHECK(events[i].type == by_default[i].type);
		CHECK(events[i].first_word == by_default[i].first_word);
	}
	for (std::size_t i = 0; i < count; ++i)
	{
		CHECK(events[i].type != id<kymo::opsy::enter_pend_sv>());
		CHECK(events[i].type != id<kymo::opsy::enter_service_call>());
		CHECK(events[i].type != id<kymo::opsy::exit_service_call>());
	}

	// Kernel enabled: PendSV switches to the worker.
	kymo::enable(kymo::family::kernel);
	count = wake_once(events, 16);
	kymo::disable(kymo::family::kernel);
	CHECK(count >= 6);
	read_event const with_kernel[] = {
		isr_in, notified, ready, isr_out, {id<kymo::opsy::enter_pend_sv>(), events[4].first_word}, started,
	};
	for (std::size_t i = 0; i < 6; ++i)
	{
		CHECK(events[i].type == with_kernel[i].type);
		CHECK(events[i].first_word == with_kernel[i].first_word);
	}

	opsy::cortex_m::disable_interrupt(g_irq);
	(void) g_worker.kill();
	for (int guard = 0; guard < 50 && g_worker.is_started(); ++guard)
		opsy::sleep_for(1ms);
}

} // namespace
