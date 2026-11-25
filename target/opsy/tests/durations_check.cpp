/**
 * @file    durations_check.cpp
 * @brief   Durations of the application across OpSy's contexts: a case of
 *          OpSy's on-target suite, linked into its image.
 *
 * A task begins a handover, nests two jobs in it, and ends without closing
 * the handover; an interrupt handler closes it afterwards. A duration belongs
 * to no context: the host must pair the handover's begin, in the task, with
 * its end, in the handler, across the task's end (trace.py, oracle.py).
 */

#include "qemu_test.hpp"

#include <opsy_trace.hpp>

#include <cortex_m.hpp>
#include <hooks.hpp>
#include <opsy.hpp>
#include <scheduler.hpp>
#include <task.hpp>

#include <atomic>
#include <chrono>
#include <cstdint>

/// Begun by a task, ended by an interrupt handler.
struct handover
{
	std::uint32_t item;
};

/// Nested in the handover, within the task.
struct job
{
	std::uint16_t number;
};

namespace
{

opsy::task<1024> g_owner;
std::atomic<int> g_handed = 0;
unsigned g_irq = qemu_test::no_irq;

void end_handover()
{
	trace::end<handover>();
	++g_handed;
}

OPSY_QEMU_TEST(a_duration_begun_by_a_task_ends_in_an_interrupt)
{
	g_handed = 0;
	CHECK(g_owner.start([] {
		trace::begin(handover{42});
		auto const outer = trace::scope(job{1});
		auto const inner = trace::scope(job{2});
	}));
	for (int guard = 0; guard < 50 && g_owner.is_started(); ++guard)
		opsy::sleep_for(std::chrono::milliseconds{1});
	CHECK(!g_owner.is_started());

	if (g_irq == qemu_test::no_irq)
		g_irq = qemu_test::find_usable_irq();
	CHECK(g_irq != qemu_test::no_irq);
	opsy::cortex_m::set_priority(g_irq, opsy::scheduler::os_callable_priority);
	opsy::cortex_m::set_isr_handler(g_irq, opsy::hooked_isr::wrap<&end_handover>);
	opsy::cortex_m::enable_interrupt(g_irq);
	qemu_test::raise(g_irq);
	CHECK(g_handed == 1);
	opsy::cortex_m::disable_interrupt(g_irq);
}

} // namespace
