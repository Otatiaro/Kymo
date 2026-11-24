/**
 * @file    events.hpp
 * @brief   The FreeRTOS adapter's events, kymo::freertos::<macro>: each named
 *          after the trace macro that emits it (trace.h), lower case and
 *          without its "trace" prefix.
 *
 * Every field is a full word: no event has padding to clear. A handle is
 * the kernel's own type, so that the host names it from the ELF. An event
 * without a task concerns the task that is running.
 *
 * Several macros emit the same event: traceTASK_RESUME and
 * traceTASK_RESUME_FROM_ISR task_resume, traceQUEUE_SEND and
 * traceQUEUE_SEND_FROM_ISR queue_send, the three notification macros
 * task_notify, traceISR_EXIT and traceISR_EXIT_TO_SCHEDULER isr_exit. Whether
 * a handler emitted it is in the trace already.
 *
 * Requires FreeRTOS's include directory and the application's
 * FreeRTOSConfig.h on the include path.
 */
#pragma once

#include <kymo/control.hpp>

// clang-format off: FreeRTOS.h before the kernel's other headers
#include <FreeRTOS.h>   // IWYU pragma: keep: configMAX_TASK_NAME_LEN, from the configuration it includes
#include <event_groups.h>
#include <queue.h>
#include <stream_buffer.h>
#include <task.h>
// clang-format on

#include <array>
#include <cstdint>

namespace kymo::freertos
{

using task_t = TaskHandle_t;
using queue_t = QueueHandle_t;
using event_group_t = EventGroupHandle_t;
using stream_buffer_t = StreamBufferHandle_t;
using handler_t = void (*)();

// Scheduling

/// The scheduler starts.
struct starting
{
	static constexpr family kymo_family = family::scheduling;
	std::uint32_t core_clock; ///< configCPU_CLOCK_HZ
	task_t idle;              ///< the idle task, which the host does not count as a task
};

/// A task is created.
struct task_create
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
	std::uint32_t priority; ///< higher is higher
	std::array<char, configMAX_TASK_NAME_LEN> name;
};

/// A task is deleted.
struct task_delete
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
};

/// A task takes the core, the idle task included.
struct task_switched_in
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
};

/// vTaskPrioritySet gives a task its base priority.
struct task_priority_set
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
	std::uint32_t priority;
};

/// A mutex holder inherits the priority of a task blocked on it.
struct task_priority_inherit
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
	std::uint32_t priority; ///< the one it inherits
};

/// A mutex holder's inherited priority drops.
struct task_priority_disinherit
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
	std::uint32_t priority; ///< the one it drops to
};

// Causality

/// A task is put in the ready list: made ready, or moved within it when its
/// priority changes while it is ready.
struct moved_task_to_ready_state
{
	static constexpr family kymo_family = family::causality;
	task_t task;
};

/// vTaskDelay.
struct task_delay
{
	static constexpr family kymo_family = family::causality;
	std::uint32_t ticks;
};

/// The running task waits until a tick count: xTaskDelayUntil, or a wait of
/// the timer task.
struct task_delay_until
{
	static constexpr family kymo_family = family::causality;
	std::uint32_t time; ///< in ticks
};

/// A task is suspended.
struct task_suspend
{
	static constexpr family kymo_family = family::causality;
	task_t task;
};

/// A suspended task is resumed.
struct task_resume
{
	static constexpr family kymo_family = family::causality;
	task_t task;
};

/// The running task blocks until a queue has an item.
struct blocking_on_queue_receive
{
	static constexpr family kymo_family = family::causality;
	queue_t queue;
};

/// The running task blocks until a queue has room.
struct blocking_on_queue_send
{
	static constexpr family kymo_family = family::causality;
	queue_t queue;
};

/// The running task blocks until a queue has an item to peek at.
struct blocking_on_queue_peek
{
	static constexpr family kymo_family = family::causality;
	queue_t queue;
};

/// The running task blocks in ulTaskNotifyTake.
struct task_notify_take_block
{
	static constexpr family kymo_family = family::causality;
	std::uint32_t index; ///< of the notification
};

/// The running task blocks in xTaskNotifyWait.
struct task_notify_wait_block
{
	static constexpr family kymo_family = family::causality;
	std::uint32_t index; ///< of the notification
};

/// The running task blocks in xEventGroupSync.
struct event_group_sync_block
{
	static constexpr family kymo_family = family::causality;
	event_group_t group;
};

/// The running task blocks in xEventGroupWaitBits.
struct event_group_wait_bits_block
{
	static constexpr family kymo_family = family::causality;
	event_group_t group;
};

/// The running task blocks until a stream buffer has room.
struct blocking_on_stream_buffer_send
{
	static constexpr family kymo_family = family::causality;
	stream_buffer_t stream;
};

/// The running task blocks until a stream buffer has data.
struct blocking_on_stream_buffer_receive
{
	static constexpr family kymo_family = family::causality;
	stream_buffer_t stream;
};

/// An item is sent to a queue, or a semaphore given, from a task or a
/// handler: the sender is the waker of a task it unblocks.
struct queue_send
{
	static constexpr family kymo_family = family::causality;
	queue_t queue;
};

/// A task is notified, from a task or a handler.
struct task_notify
{
	static constexpr family kymo_family = family::causality;
	task_t task;
};

// Exclusion

/// The running task takes a mutex: the first take of a recursive one.
struct mutex_taken
{
	static constexpr family kymo_family = family::exclusion;
	queue_t mutex;
};

/// The running task releases a mutex: the last give of a recursive one.
struct mutex_released
{
	static constexpr family kymo_family = family::exclusion;
	queue_t mutex;
};

/// The running task blocks on a mutex another task holds.
struct mutex_blocked
{
	static constexpr family kymo_family = family::exclusion;
	queue_t mutex;
};

// Interrupts

/// A handler is entered: the exception IPSR holds, and the handler the
/// vector table VTOR points to holds for it.
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

} // namespace kymo::freertos
