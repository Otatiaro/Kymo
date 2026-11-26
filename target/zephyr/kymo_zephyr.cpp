/**
 * @file    kymo_zephyr.cpp
 * @brief   The Zephyr adapter's unit: the functions trace.h's hooks call, each
 *          emitting its event (events.hpp) through the application's tracer,
 *          the kernel's interrupt trace functions, and the start.
 *
 * The application gives this unit its tracer in a header of its own,
 * kymo_zephyr_trace.hpp, on the include path it adds to the kymo_zephyr
 * library:
 *
 *     #include "trace.hpp" // the application's tracer
 *
 *     namespace kymo::zephyr
 *     {
 *     using trace = ::trace;
 *     }
 */
#include <kymo/zephyr/events.hpp>
#include <kymo/zephyr/trace.h>

#include <kymo_zephyr_trace.hpp>

#include <zephyr/init.h>
#include <zephyr/kernel.h>
#include <zephyr/sw_isr_table.h>

#include <algorithm>
#include <cstdint>

extern "C"
{
/// The idle threads, one per core (kernel/include/kthread.h).
extern struct k_thread z_idle_threads[];
}

namespace kymo::zephyr
{
namespace
{

// The kernel passes its objects as pointers to const; the casts give the
// events the handle types.
// NOLINTBEGIN(cppcoreguidelines-pro-type-const-cast)
template<typename Object>
Object* handle(Object const* object)
{
	return const_cast<Object*>(object);
}
// NOLINTEND(cppcoreguidelines-pro-type-const-cast)

name_t name_of(char const* name)
{
	name_t copy{};
	std::copy_n(name, CONFIG_THREAD_MAX_NAME_LEN, copy.begin());
	return copy;
}

/// Address of VTOR, the vector table offset register (ARMv7-M and ARMv8-M).
constexpr std::uintptr_t vtor = 0xE000ED08;
/// The first exception that is an interrupt.
constexpr std::uint32_t first_irq = 16;

/** @brief The exception the core is handling, IPSR as read: 0 is thread mode. */
std::uint32_t exception()
{
	std::uint32_t ipsr{};
	asm volatile("mrs %0, ipsr" : "=r"(ipsr));
	return ipsr;
}

/** @brief The kernel starts: the idle thread, before the threads exist. */
int kymo_zephyr_start()
{
	trace::emit(starting{sys_clock_hw_cycles_per_sec(), &z_idle_threads[0]});
	return 0;
}

SYS_INIT(kymo_zephyr_start, PRE_KERNEL_1, 0);

} // namespace

extern "C"
{

void kymo_zephyr_thread_create(k_thread const* thread, std::int32_t priority, char const* name)
{
	trace::emit(thread_create{handle(thread), priority, name_of(name)});
}

void kymo_zephyr_thread_name_set(k_thread const* thread, char const* name)
{
	trace::emit(thread_name_set{handle(thread), name_of(name)});
}

void kymo_zephyr_thread_sched_abort(k_thread const* thread)
{
	trace::emit(thread_sched_abort{handle(thread)});
}

void kymo_zephyr_thread_switched_in(k_thread const* thread)
{
	trace::emit(thread_switched_in{handle(thread)});
}

void kymo_zephyr_thread_sched_priority_set(k_thread const* thread, std::int32_t priority)
{
	trace::emit(thread_sched_priority_set{handle(thread), priority});
}

void kymo_zephyr_thread_sched_ready(k_thread const* thread)
{
	trace::emit(thread_sched_ready{handle(thread)});
}

void kymo_zephyr_thread_sleep_ticks_enter(std::int32_t ticks)
{
	trace::emit(thread_sleep_ticks_enter{ticks});
}

void kymo_zephyr_sem_take_blocking(k_sem const* sem)
{
	trace::emit(sem_take_blocking{handle(sem)});
}

void kymo_zephyr_msgq_get_blocking(k_msgq const* msgq)
{
	trace::emit(msgq_get_blocking{handle(msgq)});
}

void kymo_zephyr_msgq_put_blocking(k_msgq const* msgq)
{
	trace::emit(msgq_put_blocking{handle(msgq)});
}

void kymo_zephyr_event_wait_blocking(k_event const* event)
{
	trace::emit(event_wait_blocking{handle(event)});
}

void kymo_zephyr_pipe_read_blocking(k_pipe const* pipe)
{
	trace::emit(pipe_read_blocking{handle(pipe)});
}

void kymo_zephyr_pipe_write_blocking(k_pipe const* pipe)
{
	trace::emit(pipe_write_blocking{handle(pipe)});
}

void kymo_zephyr_mutex_lock_blocking(k_mutex const* mutex)
{
	trace::emit(mutex_lock_blocking{handle(mutex)});
}

void kymo_zephyr_mutex_taken(k_mutex const* mutex)
{
	trace::emit(mutex_taken{handle(mutex)});
}

void kymo_zephyr_mutex_released(k_mutex const* mutex)
{
	trace::emit(mutex_released{handle(mutex)});
}

void sys_trace_isr_enter()
{
	std::uint32_t const number = exception();
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): VTOR, a register at a fixed address
	auto const* table = reinterpret_cast<handler_t const*>(*reinterpret_cast<std::uint32_t const volatile*>(vtor));
	// NOLINTNEXTLINE(cppcoreguidelines-pro-bounds-pointer-arithmetic): the table's length is the part's
	handler_t handler = table[number];
#ifdef CONFIG_GEN_SW_ISR_TABLE_ARRAY
	if (handler == &_isr_wrapper && number >= first_irq)
		// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,cppcoreguidelines-pro-bounds-constant-array-index): the ISR as the host names it, by its address; the table has an entry per interrupt
		handler = reinterpret_cast<handler_t>(_sw_isr_table[number - first_irq].isr);
#endif
	trace::emit(isr_enter{number, handler});
}

void sys_trace_isr_exit()
{
	trace::emit(isr_exit{exception()});
}

/** Keeps the core awake in the idle thread (trace.h). */
bool z_arm_on_enter_cpu_idle()
{
	return false;
}

} // extern "C"

} // namespace kymo::zephyr
