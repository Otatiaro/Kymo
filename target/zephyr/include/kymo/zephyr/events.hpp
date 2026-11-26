/**
 * @file    events.hpp
 * @brief   The Zephyr adapter's events, kymo::zephyr::<hook>: each named after
 *          the tracing hook that emits it (trace.h), without its
 *          sys_port_trace_k_ prefix.
 *
 * Every field is a full word: no event has padding to clear. A handle is the
 * kernel's own type, so that the host names it from the ELF. An event without
 * a thread concerns the thread that is running.
 *
 * Three events are not a hook's: starting, which Zephyr has no hook for, and
 * mutex_taken and mutex_released, which the lock and unlock exits emit only
 * when the mutex changes hands (trace.h).
 */
#pragma once

#include <kymo/control.hpp>

#include <zephyr/kernel.h>

#include <array>
#include <cstdint>

namespace kymo::zephyr
{

using thread_t = k_thread*;
using mutex_t = k_mutex*;
using sem_t = k_sem*;
using msgq_t = k_msgq*;
using event_t = k_event*;
using pipe_t = k_pipe*;
using handler_t = void (*)();

/// A thread's name, in whole words.
using name_t = std::array<char, (CONFIG_THREAD_MAX_NAME_LEN + 3) / 4 * 4>;

// Scheduling

/// The kernel starts, before it creates its threads.
struct starting
{
	static constexpr family kymo_family = family::scheduling;
	std::uint32_t core_clock; ///< sys_clock_hw_cycles_per_sec()
	thread_t idle;            ///< the idle thread, which the host does not count as a thread
};

/// A thread is created.
struct thread_create
{
	static constexpr family kymo_family = family::scheduling;
	thread_t thread;
	std::int32_t priority; ///< lower is higher; a negative one is cooperative
	name_t name;
};

/// A thread is named.
struct thread_name_set
{
	static constexpr family kymo_family = family::scheduling;
	thread_t thread;
	name_t name;
};

/// A thread ends: it returned, or was aborted.
struct thread_sched_abort
{
	static constexpr family kymo_family = family::scheduling;
	thread_t thread;
};

/// A thread takes the core, the idle thread included.
struct thread_switched_in
{
	static constexpr family kymo_family = family::scheduling;
	thread_t thread;
};

/// A thread's priority changes: k_thread_priority_set, or a mutex holder
/// inheriting the priority of a thread blocked on it, and dropping back.
struct thread_sched_priority_set
{
	static constexpr family kymo_family = family::scheduling;
	thread_t thread;
	std::int32_t priority;
};

// Causality

/// A thread is put in the run queue: made ready.
struct thread_sched_ready
{
	static constexpr family kymo_family = family::causality;
	thread_t thread;
};

/// The running thread sleeps: k_sleep and its kin.
struct thread_sleep_ticks_enter
{
	static constexpr family kymo_family = family::causality;
	std::int32_t ticks; ///< -1 for ever, 0 a yield
};

/// The running thread blocks until a semaphore is given.
struct sem_take_blocking
{
	static constexpr family kymo_family = family::causality;
	sem_t sem;
};

/// The running thread blocks until a message queue has a message.
struct msgq_get_blocking
{
	static constexpr family kymo_family = family::causality;
	msgq_t msgq;
};

/// The running thread blocks until a message queue has room.
struct msgq_put_blocking
{
	static constexpr family kymo_family = family::causality;
	msgq_t msgq;
};

/// The running thread blocks until an event's bits are posted.
struct event_wait_blocking
{
	static constexpr family kymo_family = family::causality;
	event_t event;
};

/// The running thread blocks until a pipe has data.
struct pipe_read_blocking
{
	static constexpr family kymo_family = family::causality;
	pipe_t pipe;
};

/// The running thread blocks until a pipe has room.
struct pipe_write_blocking
{
	static constexpr family kymo_family = family::causality;
	pipe_t pipe;
};

// Exclusion

/// The running thread blocks on a mutex another thread holds.
struct mutex_lock_blocking
{
	static constexpr family kymo_family = family::exclusion;
	mutex_t mutex;
};

/// The running thread takes a mutex: the first lock of a recursive one.
struct mutex_taken
{
	static constexpr family kymo_family = family::exclusion;
	mutex_t mutex;
};

/// The running thread releases a mutex: the last unlock of a recursive one.
struct mutex_released
{
	static constexpr family kymo_family = family::exclusion;
	mutex_t mutex;
};

// Interrupts

/// A handler is entered: the exception IPSR holds, and the handler that
/// serves it (trace.h).
struct isr_enter
{
	static constexpr family kymo_family = family::interrupts;
	std::uint32_t exception; ///< IPSR: 16 + the IRQ number
	handler_t handler;
};

/// A handler returns.
struct isr_exit
{
	static constexpr family kymo_family = family::interrupts;
	std::uint32_t exception; ///< IPSR
};

} // namespace kymo::zephyr
