/**
 * @file    trace.h
 * @brief   The FreeRTOS adapter's trace macros: FreeRTOSConfig.h includes this
 *          header, last, and the kernel's trace points call the adapter.
 *
 * Each macro calls a function of the adapter's unit (kymo_freertos.cpp),
 * which emits the event named after the macro, kymo::freertos::<macro>, in
 * the application's tracer. Link-time optimisation can inline the calls.
 *
 * The macros pass what the kernel has in scope where it expands them: a task
 * is its handle, a queue, mutex, event group or stream buffer its handle; the
 * host names them from the ELF, and a task also from the name task_create
 * carries. An event without a task concerns the task that is running.
 *
 * A mutex is a queue to FreeRTOS. The queue macros tell one from the other on
 * the queue's type, uxQueueType == queueQUEUE_IS_MUTEX (queue.c), which holds
 * for a recursive mutex as well: the host receives mutex_taken,
 * mutex_released and mutex_blocked, never a queue event for a mutex. A
 * recursive mutex is taken by its first take and released by its last give,
 * the only ones that go through the queue; the nested ones are not recorded.
 *
 * Interrupts: isr_enter at traceISR_ENTER, isr_exit at traceISR_EXIT and
 * traceISR_EXIT_TO_SCHEDULER. The port's SysTick handler calls them. An
 * application's handler calls traceISR_ENTER first; portYIELD_FROM_ISR and
 * portEND_SWITCHING_ISR call the exit, and a handler that ends without them
 * calls traceISR_EXIT itself, once. The handler is read at entry from the
 * vector table VTOR points to, at the index IPSR gives: a vector changed at
 * run time is named as it is. Cortex-M0, which has no VTOR, is not
 * supported.
 *
 * Tracing needs configUSE_TICKLESS_IDLE 0: a sleeping core makes some parts
 * read the whole system bus, the ring included, as zeros to the probe.
 *
 * A task woken while the scheduler is suspended (vTaskSuspendAll) waits in
 * the pending ready list, which no macro reports: its
 * moved_task_to_ready_state comes at xTaskResumeAll, and its wake latency
 * reads that much shorter.
 *
 * Faults: FreeRTOS has no fault hook. The application's fault handler calls
 * kymo_freertos_fault() with the frame the core stacked and the EXC_RETURN
 * it was entered with, before anything else; it records a kymo::fault
 * (kymo/fault.hpp) and returns. What comes next, kymo::freeze(), a reset,
 * is the handler's. A handler in assembly passes the stack EXC_RETURN names:
 *
 *     tst   lr, #4
 *     ite   eq
 *     mrseq r0, msp
 *     mrsne r0, psp
 *     mov   r1, lr
 *     b     application_fault   @ which calls kymo_freertos_fault(r0, r1)
 *
 * The event's call levels come from up to 64 words read above the frame. On
 * the main stack they stop at its start, from the vector table. A task's
 * stack is the kernel's, its bounds not known here: the 64 words are read
 * whatever they are. The application keeps readable the 256 bytes above the
 * start (highest address) of every task stack: no task stack starts less than
 * 256 bytes before the end of its RAM.
 */
#ifndef KYMO_FREERTOS_TRACE_H
#define KYMO_FREERTOS_TRACE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void kymo_freertos_starting(void const* idle);
void kymo_freertos_task_create(void const* task, uint32_t priority, char const* name);
void kymo_freertos_task_delete(void const* task);
void kymo_freertos_task_switched_in(void const* task);
void kymo_freertos_task_priority_set(void const* task, uint32_t priority);
void kymo_freertos_task_priority_inherit(void const* task, uint32_t priority);
void kymo_freertos_task_priority_disinherit(void const* task, uint32_t priority);

void kymo_freertos_moved_task_to_ready_state(void const* task);
void kymo_freertos_task_delay(uint32_t ticks);
void kymo_freertos_task_delay_until(uint32_t time);
void kymo_freertos_task_suspend(void const* task);
void kymo_freertos_task_resume(void const* task);
void kymo_freertos_blocking_on_queue_receive(void const* queue);
void kymo_freertos_blocking_on_queue_send(void const* queue);
void kymo_freertos_blocking_on_queue_peek(void const* queue);
void kymo_freertos_task_notify_take_block(uint32_t index);
void kymo_freertos_task_notify_wait_block(uint32_t index);
void kymo_freertos_event_group_sync_block(void const* group);
void kymo_freertos_event_group_wait_bits_block(void const* group);
void kymo_freertos_blocking_on_stream_buffer_send(void const* stream);
void kymo_freertos_blocking_on_stream_buffer_receive(void const* stream);
void kymo_freertos_queue_send(void const* queue);
void kymo_freertos_task_notify(void const* task);

void kymo_freertos_mutex_taken(void const* mutex);
void kymo_freertos_mutex_released(void const* mutex);
void kymo_freertos_mutex_blocked(void const* mutex);

void kymo_freertos_isr_enter(void);
void kymo_freertos_isr_exit(void);

void kymo_freertos_fault(uint32_t const* frame, uint32_t exc_return);

#ifdef __cplusplus
}
#endif

// Scheduling

#define traceSTARTING_SCHEDULER(xIdleTaskHandles) kymo_freertos_starting((xIdleTaskHandles)[0])
#define traceTASK_CREATE(pxNewTCB) kymo_freertos_task_create((pxNewTCB), (pxNewTCB)->uxPriority, (pxNewTCB)->pcTaskName)
#define traceTASK_DELETE(pxTaskToDelete) kymo_freertos_task_delete(pxTaskToDelete)
#define traceTASK_SWITCHED_IN() kymo_freertos_task_switched_in(pxCurrentTCB)
#define traceTASK_PRIORITY_SET(pxTask, uxNewPriority) kymo_freertos_task_priority_set((pxTask), (uxNewPriority))
#define traceTASK_PRIORITY_INHERIT(pxTCBOfMutexHolder, uxInheritedPriority) \
	kymo_freertos_task_priority_inherit((pxTCBOfMutexHolder), (uxInheritedPriority))
#define traceTASK_PRIORITY_DISINHERIT(pxTCBOfMutexHolder, uxOriginalPriority) \
	kymo_freertos_task_priority_disinherit((pxTCBOfMutexHolder), (uxOriginalPriority))

// Causality

#define traceMOVED_TASK_TO_READY_STATE(pxTCB) kymo_freertos_moved_task_to_ready_state(pxTCB)
/* vTaskDelay's argument, in scope where the kernel expands the macro. */
#define traceTASK_DELAY() kymo_freertos_task_delay(xTicksToDelay)
#define traceTASK_DELAY_UNTIL(xTimeToWake) kymo_freertos_task_delay_until(xTimeToWake)
#define traceTASK_SUSPEND(pxTaskToSuspend) kymo_freertos_task_suspend(pxTaskToSuspend)
#define traceTASK_RESUME(pxTaskToResume) kymo_freertos_task_resume(pxTaskToResume)
#define traceTASK_RESUME_FROM_ISR(pxTaskToResume) kymo_freertos_task_resume(pxTaskToResume)
#define traceBLOCKING_ON_QUEUE_SEND(pxQueue) kymo_freertos_blocking_on_queue_send(pxQueue)
#define traceBLOCKING_ON_QUEUE_PEEK(pxQueue) kymo_freertos_blocking_on_queue_peek(pxQueue)
#define traceTASK_NOTIFY_TAKE_BLOCK(uxIndexToWait) kymo_freertos_task_notify_take_block(uxIndexToWait)
#define traceTASK_NOTIFY_WAIT_BLOCK(uxIndexToWait) kymo_freertos_task_notify_wait_block(uxIndexToWait)
#define traceEVENT_GROUP_SYNC_BLOCK(xEventGroup, uxBitsToSet, uxBitsToWaitFor) \
	kymo_freertos_event_group_sync_block(xEventGroup)
#define traceEVENT_GROUP_WAIT_BITS_BLOCK(xEventGroup, uxBitsToWaitFor) \
	kymo_freertos_event_group_wait_bits_block(xEventGroup)
#define traceBLOCKING_ON_STREAM_BUFFER_SEND(xStreamBuffer) kymo_freertos_blocking_on_stream_buffer_send(xStreamBuffer)
#define traceBLOCKING_ON_STREAM_BUFFER_RECEIVE(xStreamBuffer) \
	kymo_freertos_blocking_on_stream_buffer_receive(xStreamBuffer)
#define traceQUEUE_SEND_FROM_ISR(pxQueue) kymo_freertos_queue_send(pxQueue)
/* The task notified, in scope where the kernel expands these macros. */
#define traceTASK_NOTIFY(uxIndexToNotify) kymo_freertos_task_notify(pxTCB)
#define traceTASK_NOTIFY_FROM_ISR(uxIndexToNotify) kymo_freertos_task_notify(pxTCB)
#define traceTASK_NOTIFY_GIVE_FROM_ISR(uxIndexToNotify) kymo_freertos_task_notify(pxTCB)

// Queues and mutexes. A give finds the mutex still held by the task giving
// it; the give that creates a mutex finds it held by none, and is not
// recorded. A take finds it not held yet.

#define traceQUEUE_SEND(pxQueue)                                               \
	do                                                                         \
	{                                                                          \
		if ((pxQueue)->uxQueueType != queueQUEUE_IS_MUTEX)                     \
			kymo_freertos_queue_send(pxQueue);                                 \
		else if ((pxQueue)->u.xSemaphore.xMutexHolder != NULL)                 \
			kymo_freertos_mutex_released(pxQueue);                             \
	} while (0)
#define traceQUEUE_RECEIVE(pxQueue)                                            \
	do                                                                         \
	{                                                                          \
		if ((pxQueue)->uxQueueType == queueQUEUE_IS_MUTEX)                     \
			kymo_freertos_mutex_taken(pxQueue);                                \
	} while (0)
#define traceBLOCKING_ON_QUEUE_RECEIVE(pxQueue)                                \
	do                                                                         \
	{                                                                          \
		if ((pxQueue)->uxQueueType == queueQUEUE_IS_MUTEX)                     \
			kymo_freertos_mutex_blocked(pxQueue);                              \
		else                                                                   \
			kymo_freertos_blocking_on_queue_receive(pxQueue);                  \
	} while (0)

// Interrupts

#define traceISR_ENTER() kymo_freertos_isr_enter()
#define traceISR_EXIT() kymo_freertos_isr_exit()
#define traceISR_EXIT_TO_SCHEDULER() kymo_freertos_isr_exit()

#endif
