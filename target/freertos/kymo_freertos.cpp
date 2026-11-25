/**
 * @file    kymo_freertos.cpp
 * @brief   The FreeRTOS adapter's unit: the functions trace.h's macros call,
 *          each emitting its event (events.hpp) through the application's
 *          tracer.
 *
 * The application builds this unit with its sources, and gives it its
 * tracer in a header of its own on the include path, kymo_freertos_trace.hpp:
 *
 *     #include "trace.hpp" // the application's tracer
 *
 *     namespace kymo::freertos
 *     {
 *     using trace = ::trace;
 *     }
 */
#include <kymo/fault.hpp>
#include <kymo/freertos/events.hpp>
#include <kymo/freertos/trace.h>

#include <kymo_freertos_trace.hpp>

#include <algorithm>
#include <cstdint>

namespace kymo::freertos
{
namespace
{

// The kernel passes a handle as a pointer to its own control block, which
// is the handle's target; the casts give it back its type.
// NOLINTBEGIN(cppcoreguidelines-pro-type-const-cast)
task_t task_of(void const* task)
{
	return static_cast<task_t>(const_cast<void*>(task));
}

queue_t queue_of(void const* queue)
{
	return static_cast<queue_t>(const_cast<void*>(queue));
}
// NOLINTEND(cppcoreguidelines-pro-type-const-cast)

/// Address of VTOR, the vector table offset register (ARMv7-M and ARMv8-M).
constexpr std::uintptr_t vtor = 0xE000ED08;

/** @brief The exception the core is handling, IPSR as read: 0 is thread mode. */
std::uint32_t exception()
{
	std::uint32_t ipsr{};
	asm volatile("mrs %0, ipsr" : "=r"(ipsr));
	return ipsr;
}

} // namespace

extern "C"
{

void kymo_freertos_starting(void const* idle)
{
	trace::emit(starting{configCPU_CLOCK_HZ, task_of(idle)});
}

void kymo_freertos_task_create(void const* task, std::uint32_t priority, char const* name)
{
	task_create event{task_of(task), priority, {}};
	std::copy_n(name, event.name.size(), event.name.begin());
	trace::emit(event);
}

void kymo_freertos_task_delete(void const* task)
{
	trace::emit(task_delete{task_of(task)});
}

void kymo_freertos_task_switched_in(void const* task)
{
	trace::emit(task_switched_in{task_of(task)});
}

void kymo_freertos_task_priority_set(void const* task, std::uint32_t priority)
{
	trace::emit(task_priority_set{task_of(task), priority});
}

void kymo_freertos_task_priority_inherit(void const* task, std::uint32_t priority)
{
	trace::emit(task_priority_inherit{task_of(task), priority});
}

void kymo_freertos_task_priority_disinherit(void const* task, std::uint32_t priority)
{
	trace::emit(task_priority_disinherit{task_of(task), priority});
}

void kymo_freertos_moved_task_to_ready_state(void const* task)
{
	trace::emit(moved_task_to_ready_state{task_of(task)});
}

void kymo_freertos_task_delay(std::uint32_t ticks)
{
	trace::emit(task_delay{ticks});
}

void kymo_freertos_task_delay_until(std::uint32_t time)
{
	trace::emit(task_delay_until{time});
}

void kymo_freertos_task_suspend(void const* task)
{
	trace::emit(task_suspend{task_of(task)});
}

void kymo_freertos_task_resume(void const* task)
{
	trace::emit(task_resume{task_of(task)});
}

void kymo_freertos_blocking_on_queue_receive(void const* queue)
{
	trace::emit(blocking_on_queue_receive{queue_of(queue)});
}

void kymo_freertos_blocking_on_queue_send(void const* queue)
{
	trace::emit(blocking_on_queue_send{queue_of(queue)});
}

void kymo_freertos_blocking_on_queue_peek(void const* queue)
{
	trace::emit(blocking_on_queue_peek{queue_of(queue)});
}

void kymo_freertos_task_notify_take_block(std::uint32_t index)
{
	trace::emit(task_notify_take_block{index});
}

void kymo_freertos_task_notify_wait_block(std::uint32_t index)
{
	trace::emit(task_notify_wait_block{index});
}

// NOLINTBEGIN(cppcoreguidelines-pro-type-const-cast)
void kymo_freertos_event_group_sync_block(void const* group)
{
	trace::emit(event_group_sync_block{static_cast<event_group_t>(const_cast<void*>(group))});
}

void kymo_freertos_event_group_wait_bits_block(void const* group)
{
	trace::emit(event_group_wait_bits_block{static_cast<event_group_t>(const_cast<void*>(group))});
}

void kymo_freertos_blocking_on_stream_buffer_send(void const* stream)
{
	trace::emit(blocking_on_stream_buffer_send{static_cast<stream_buffer_t>(const_cast<void*>(stream))});
}

void kymo_freertos_blocking_on_stream_buffer_receive(void const* stream)
{
	trace::emit(blocking_on_stream_buffer_receive{static_cast<stream_buffer_t>(const_cast<void*>(stream))});
}
// NOLINTEND(cppcoreguidelines-pro-type-const-cast)

void kymo_freertos_queue_send(void const* queue)
{
	trace::emit(queue_send{queue_of(queue)});
}

void kymo_freertos_task_notify(void const* task)
{
	trace::emit(task_notify{task_of(task)});
}

void kymo_freertos_mutex_taken(void const* mutex)
{
	trace::emit(mutex_taken{queue_of(mutex)});
}

void kymo_freertos_mutex_released(void const* mutex)
{
	trace::emit(mutex_released{queue_of(mutex)});
}

void kymo_freertos_mutex_blocked(void const* mutex)
{
	trace::emit(mutex_blocked{queue_of(mutex)});
}

void kymo_freertos_isr_enter()
{
	std::uint32_t const number = exception();
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): VTOR, a register at a fixed address
	auto const* table = reinterpret_cast<handler_t const*>(*reinterpret_cast<std::uint32_t const volatile*>(vtor));
	// NOLINTNEXTLINE(cppcoreguidelines-pro-bounds-pointer-arithmetic): the table's length is the part's
	trace::emit(isr_enter{number, table[number]});
}

void kymo_freertos_isr_exit()
{
	trace::emit(isr_exit{exception()});
}

void kymo_freertos_fault(std::uint32_t const* frame, std::uint32_t exc_return)
{
	constexpr std::uint32_t process_stack = 1U << 2; // EXC_RETURN.SPSEL
	constexpr std::uintptr_t cfsr = 0xE000ED28;
	constexpr std::uintptr_t hfsr = 0xE000ED2C;
	constexpr std::uintptr_t mmfar = 0xE000ED34;
	constexpr std::uintptr_t bfar = 0xE000ED38;
	// NOLINTBEGIN(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr,cppcoreguidelines-pro-bounds-pointer-arithmetic,cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers): the fault registers at their fixed addresses, the frame the core stacked in its order
	auto const status = [](std::uintptr_t address) { return *reinterpret_cast<std::uint32_t const volatile*>(address); };
	auto const frame_address = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(frame));

	// The main stack starts where the vector table says. A task's stack is
	// FreeRTOS's own: its top is not known, and the 64 words past the frame
	// are assumed readable.
	std::uint32_t const top = (exc_return & process_stack) == 0 ? main_stack_top() : 0;
	std::uint32_t const limit = top != 0 ? top : UINT32_MAX;

	fault event{};
	event.cfsr = status(cfsr);
	event.hfsr = status(hfsr);
	event.mmfar = status(mmfar);
	event.bfar = status(bfar);
	event.r0 = frame[0];
	event.r1 = frame[1];
	event.r2 = frame[2];
	event.r3 = frame[3];
	event.r12 = frame[4];
	event.lr = frame[5];
	event.pc = frame[6];
	event.xpsr = frame[7];
	// NOLINTEND(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr,cppcoreguidelines-pro-bounds-pointer-arithmetic,cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers)
	event.exc_return = exc_return;
	event.sp = frame_address;
	event.stack_top = top;
	if (frame_address < limit)
		event.returns = fault_returns(frame_address, exc_return, limit);
	trace::emit(event);
}

} // extern "C"

} // namespace kymo::freertos
