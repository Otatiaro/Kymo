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

#include <opsy_trace.hpp>

#include <condition_variable.hpp>
#include <cortex_m.hpp>
#include <hooks.hpp>
#include <isr_lock.hpp>
#include <opsy.hpp>
#include <scheduler.hpp>
#include <task.hpp>

#include <kymo/control.hpp>
#include <kymo/kymo.hpp>
#include <kymo/opsy/hooks.hpp>
#include <kymo/ring.hpp>
#include <kymo/type_id.hpp>

#include <array>
#include <atomic>
#include <bit>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <span>
#include <type_traits>

static_assert(kymo::control_at_reset() == ((1U << kymo::families) - 1U) - kymo::bit(kymo::family::kernel), "kernel alone is off by default");

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
		std::scoped_lock const guard{g_lock};
		g_raised = true;
	}
	g_wake.notify_one();
}

struct read_event
{
	std::uint32_t type;
	std::uint32_t first_word; ///< the first payload word
};

using read_events = std::array<read_event, 16>;

/**
 * @brief The events written from @p from on, first slots only, SysTick's
 *        left out, in @p out; returns how many. A slot whose header does not
 *        carry its index's lap was not written there by this pass, or was
 *        overwritten since: it is skipped.
 */
std::size_t read_since(std::uint32_t from, std::span<read_event> out)
{
	auto& ring = trace_ring;
	using ring_t = std::remove_reference_t<decltype(ring)>;
	constexpr std::uint32_t mask = ring_slots - 1;
	constexpr std::uint32_t type_mask = kymo::head_bit - 1;
	std::uint32_t const systick_in = kymo::type_id<kymo::opsy::enter_systick>();
	std::uint32_t const systick_out = kymo::type_id<kymo::opsy::exit_systick>();

	std::size_t count = 0;
	std::uint32_t const end = ring.write_index.load(std::memory_order_acquire);
	for (std::uint32_t index = from; index != end && count < out.size(); ++index)
	{
		auto const& slot = ring.slots[index & mask];
		std::uint32_t const header = slot.header.load(std::memory_order_acquire);
		std::uint32_t const type = header & type_mask;
		if ((header & kymo::head_bit) == 0 || header != ring_t::header(type, index, true))
			continue;
		if (type == systick_in || type == systick_out)
			continue;
		out[count++] = {.type = type, .first_word = slot.words[1]};
	}
	return count;
}

template<typename Event>
std::uint32_t id()
{
	return kymo::type_id<Event>();
}

/// The address the adapter records for @p pointer, an object's or a function's.
template<typename Pointer>
std::uint32_t address(Pointer pointer)
{
	return static_cast<std::uint32_t>(std::bit_cast<std::uintptr_t>(pointer));
}

/// Raises the interrupt once, and reads back what it caused.
std::size_t wake_once(read_events& events)
{
	int const woken = g_woken;
	std::uint32_t const from = trace_ring.write_index.load(std::memory_order_relaxed);
	qemu_test::raise(g_irq);
	CHECK(g_woken == woken + 1);
	return read_since(from, events);
}

OPSY_QEMU_TEST(the_adapter_records_an_interrupt_waking_a_task)
{
	g_raised = false;
	g_woken = 0;
	CHECK(g_worker.start([] {
		while (true)
		{
			{
				std::scoped_lock const guard{g_lock};
				while (!g_raised)
					g_wake.wait(g_lock);
				g_raised = false;
			}
			++g_woken;
		}
	}));
	g_worker.priority(opsy::task_priority::highest);
	opsy::sleep_for(std::chrono::milliseconds{2});

	if (g_irq == qemu_test::no_irq)
		g_irq = qemu_test::find_usable_irq();
	CHECK(g_irq != qemu_test::no_irq);
	opsy::cortex_m::set_priority(g_irq, opsy::scheduler::os_callable_priority);
	opsy::cortex_m::set_isr_handler(g_irq, opsy::hooked_isr::wrap<&wake_worker>);
	opsy::cortex_m::enable_interrupt(g_irq);

	opsy::task_control_block const& worker = g_worker;
	read_event const isr_in{.type = id<kymo::opsy::isr_enter>(), .first_word = address(&wake_worker)};
	read_event const notified{.type = id<kymo::opsy::condition_variable_notify_one>(), .first_word = address(&g_wake)};
	read_event const ready{.type = id<kymo::opsy::task_ready>(), .first_word = address(&worker)};
	read_event const isr_out{.type = id<kymo::opsy::isr_exit>(), .first_word = address(&wake_worker)};
	read_event const started{.type = id<kymo::opsy::task_started>(), .first_word = address(&worker)};

	// The default control word: nothing of OpSy's own handlers.
	read_events events{};
	std::size_t count = wake_once(events);
	std::array const by_default{isr_in, notified, ready, isr_out, started};
	CHECK(count >= by_default.size());
	for (std::size_t i = 0; i < by_default.size(); ++i)
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
	count = wake_once(events);
	kymo::disable(kymo::family::kernel);
	read_event const pend_sv{.type = id<kymo::opsy::enter_pend_sv>(), .first_word = events[4].first_word};
	std::array const with_kernel{isr_in, notified, ready, isr_out, pend_sv, started};
	CHECK(count >= with_kernel.size());
	for (std::size_t i = 0; i < with_kernel.size(); ++i)
	{
		CHECK(events[i].type == with_kernel[i].type);
		CHECK(events[i].first_word == with_kernel[i].first_word);
	}

	opsy::cortex_m::disable_interrupt(g_irq);
	(void) g_worker.kill();
	for (int guard = 0; guard < 50 && g_worker.is_started(); ++guard)
		opsy::sleep_for(std::chrono::milliseconds{1});
}

} // namespace
