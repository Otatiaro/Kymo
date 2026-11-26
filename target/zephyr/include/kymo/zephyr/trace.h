/**
 * @file    trace.h
 * @brief   The Zephyr adapter's tracing format header: CONFIG_KYMO sets
 *          CONFIG_TRACING_FORMAT_HEADER to it, <zephyr/tracing/tracing.h>
 *          includes it, and the kernel's trace points call the adapter.
 *
 * Each hook defined here calls a function of the adapter's unit
 * (kymo_zephyr.cpp), which emits the event named after the hook,
 * kymo::zephyr::<hook> without its sys_port_trace_k_ prefix, in the
 * application's tracer. The hooks not defined here are left empty by
 * <zephyr/tracing/tracing_hooks.h>.
 *
 * The hooks pass what the kernel has in scope where it expands them: a thread
 * is its struct k_thread, a mutex, semaphore, message queue, event or pipe its
 * object; the host names them from the ELF, and a thread also from the name
 * thread_create carries. An event without a thread concerns the thread that is
 * running.
 *
 * Mutexes: a mutex is taken by its first lock and released by its last
 * unlock; the nested locks of a recursive one, and the locks and unlocks that
 * fail, are not recorded. A mutex handed to the thread waiting on it is
 * released when the unlock starts, and taken when that thread returns from its
 * lock.
 *
 * Interrupts: the kernel calls sys_trace_isr_enter() and sys_trace_isr_exit()
 * in its interrupt wrapper, in the header and footer of a direct ISR, and in
 * the SysTick driver; the adapter's unit defines them. The handler is read at
 * entry from the vector table VTOR points to, at the index IPSR gives; for an
 * interrupt the vector sends to the kernel's wrapper, from the software ISR
 * table instead, which holds the ISR the application connected. Cortex-M0,
 * which has no VTOR, is not supported.
 *
 * Idle: the idle thread is a thread, and its switches tell when it runs:
 * sys_trace_idle() and sys_trace_idle_exit() keep the kernel's defaults. The
 * adapter's z_arm_on_enter_cpu_idle() keeps the core from sleeping.
 */
#ifndef KYMO_ZEPHYR_TRACE_H
#define KYMO_ZEPHYR_TRACE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

struct k_thread;
struct k_mutex;
struct k_sem;
struct k_msgq;
struct k_event;
struct k_pipe;

void kymo_zephyr_thread_create(struct k_thread const* thread, int32_t priority, char const* name);
void kymo_zephyr_thread_name_set(struct k_thread const* thread, char const* name);
void kymo_zephyr_thread_sched_abort(struct k_thread const* thread);
void kymo_zephyr_thread_switched_in(struct k_thread const* thread);
void kymo_zephyr_thread_sched_priority_set(struct k_thread const* thread, int32_t priority);

void kymo_zephyr_thread_sched_ready(struct k_thread const* thread);
void kymo_zephyr_thread_sleep_ticks_enter(int32_t ticks);
void kymo_zephyr_sem_take_blocking(struct k_sem const* sem);
void kymo_zephyr_msgq_get_blocking(struct k_msgq const* msgq);
void kymo_zephyr_msgq_put_blocking(struct k_msgq const* msgq);
void kymo_zephyr_event_wait_blocking(struct k_event const* event);
void kymo_zephyr_pipe_read_blocking(struct k_pipe const* pipe);
void kymo_zephyr_pipe_write_blocking(struct k_pipe const* pipe);

void kymo_zephyr_mutex_lock_blocking(struct k_mutex const* mutex);
void kymo_zephyr_mutex_taken(struct k_mutex const* mutex);
void kymo_zephyr_mutex_released(struct k_mutex const* mutex);

#ifdef __cplusplus
}
#endif

// Scheduling

#define sys_port_trace_k_thread_create(new_thread) \
	kymo_zephyr_thread_create((new_thread), (new_thread)->base.prio, (new_thread)->name)
#define sys_port_trace_k_thread_name_set(thread, ret)          \
	do                                                        \
	{                                                         \
		if ((ret) == 0)                                       \
			kymo_zephyr_thread_name_set((thread), (thread)->name); \
	} while (0)
#define sys_port_trace_k_thread_sched_abort(thread) kymo_zephyr_thread_sched_abort(thread)
/* The thread switched in, in scope where the kernel expands the hook. */
#define sys_port_trace_k_thread_switched_in() kymo_zephyr_thread_switched_in(_current)
#define sys_port_trace_k_thread_sched_priority_set(thread, prio) \
	kymo_zephyr_thread_sched_priority_set((thread), (prio))

// Causality

#define sys_port_trace_k_thread_sched_ready(thread) kymo_zephyr_thread_sched_ready(thread)
#define sys_port_trace_k_thread_sleep_ticks_enter(timeout) \
	kymo_zephyr_thread_sleep_ticks_enter((int32_t)(timeout).ticks)
#define sys_port_trace_k_sem_take_blocking(sem, timeout) kymo_zephyr_sem_take_blocking(sem)
#define sys_port_trace_k_msgq_get_blocking(msgq, timeout) kymo_zephyr_msgq_get_blocking(msgq)
#define sys_port_trace_k_msgq_put_blocking(msgq, timeout) kymo_zephyr_msgq_put_blocking(msgq)
#define sys_port_trace_k_event_wait_blocking(event, events, options, timeout) \
	kymo_zephyr_event_wait_blocking(event)
#define sys_port_trace_k_pipe_read_blocking(pipe, timeout) kymo_zephyr_pipe_read_blocking(pipe)
#define sys_port_trace_k_pipe_write_blocking(pipe, timeout) kymo_zephyr_pipe_write_blocking(pipe)

// Exclusion. A lock that returns 0 holds the mutex; its first holds it once.
// An unlock releases it when the running thread owns it once: at its entry,
// since the unlock hands the mutex to a waiting thread, which may run before
// the unlock returns.

#define sys_port_trace_k_mutex_lock_blocking(mutex, timeout) kymo_zephyr_mutex_lock_blocking(mutex)
#define sys_port_trace_k_mutex_lock_exit(mutex, timeout, ret)   \
	do                                                         \
	{                                                          \
		if ((ret) == 0 && (mutex)->lock_count == 1U)           \
			kymo_zephyr_mutex_taken(mutex);                    \
	} while (0)
#define sys_port_trace_k_mutex_unlock_enter(mutex)                       \
	do                                                                  \
	{                                                                   \
		if ((mutex)->owner == _current && (mutex)->lock_count == 1U)    \
			kymo_zephyr_mutex_released(mutex);                          \
	} while (0)

#include <zephyr/tracing/tracing_default_hooks.h>
#include <zephyr/tracing/tracing_hooks.h>

#endif
