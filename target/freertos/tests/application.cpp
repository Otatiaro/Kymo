/**
 * @file    application.cpp
 * @brief   The FreeRTOS test application: the control task runs one scenario
 *          after another, each taking the kernel through trace points the
 *          adapter records, then exits through qemu::exit with its verdict.
 *          trace/trace.py records it and checks the recording.
 *
 * Priorities: idle 0, low 1, ephemeral 2, control 3, ping and pong 4,
 * receiver 5.
 *
 * Scenarios, in order:
 * 1. switches: ping and pong hand the core to each other three times;
 * 2. a handler wakes the receiver through a queue;
 * 3. a free mutex and a recursive one, taken and given by control; then a
 *    mutex low holds and control blocks on: low inherits control's priority,
 *    and drops back when it gives the mutex;
 * 4. delays of a tick, vTaskDelay and xTaskDelayUntil;
 * 5. the priority of a ready task changed, twice;
 * 6. a blocked task suspended and resumed;
 * 7. control blocks for a tick on each kind of wait: queue peek, receive and
 *    send, notification take and wait, event group wait and sync, stream
 *    buffer receive and send;
 * 8. a task created and deleted before it runs.
 *
 * The handler is installed at run time, in a vector table in RAM
 * (platform.hpp).
 */
#include "platform.hpp"
#include "qemu.hpp"

#include <FreeRTOS.h>
#include <event_groups.h>
#include <queue.h>
#include <semphr.h>
#include <stream_buffer.h>
#include <task.h>
#include <kymo/freertos/trace.h>

#include <array>
#include <cstddef>
#include <cstdint>

extern "C"
{
/** @brief The end of the test, where trace/trace.py halts the guest and reads
 *  the verdict from r0: out of line, whatever the optimisation. */
[[gnu::noinline]] void application_end(bool pass)
{
	qemu::finish(pass);
}

void application_assert(char const* file, int line)
{
	qemu::write("assertion failed: ");
	qemu::write(file);
	qemu::write(":");
	qemu::write_unsigned(static_cast<std::uint32_t>(line));
	qemu::write("\n");
	application_end(false);
}
}

namespace application
{

constexpr std::size_t stack_depth = configMINIMAL_STACK_SIZE;
/// The rounds of the switch scenario, each a switch to pong and one back.
constexpr unsigned rounds = 3;
constexpr std::uint32_t sent_by_handler = 42;

StaticTask_t control_tcb, ping_tcb, pong_tcb, receiver_tcb, low_tcb, ephemeral_tcb;
std::array<StackType_t, stack_depth> control_stack, ping_stack, pong_stack, receiver_stack, low_stack, ephemeral_stack;
TaskHandle_t control, ping, pong, receiver, low;

StaticQueue_t handler_queue_storage, empty_queue_storage, full_queue_storage;
std::array<std::uint8_t, sizeof(std::uint32_t)> handler_queue_items, empty_queue_items, full_queue_items;
QueueHandle_t handler_queue, empty_queue, full_queue;

StaticSemaphore_t free_mutex_storage, recursive_mutex_storage, contested_mutex_storage;
SemaphoreHandle_t free_mutex, recursive_mutex, contested_mutex;

StaticEventGroup_t group_storage;
EventGroupHandle_t group;

constexpr std::size_t stream_size = 4;
StaticStreamBuffer_t stream_storage;
std::array<std::uint8_t, stream_size + 1> stream_bytes;
StreamBufferHandle_t stream;

/// What low does when control notifies it.
enum class job : std::uint8_t
{
	nothing,
	hold_contested_mutex,
};
job volatile low_job = job::nothing;

unsigned volatile bounces = 0;
std::uint32_t volatile received = 0;

/// A handler enters with traceISR_ENTER; portYIELD_FROM_ISR exits with
/// traceISR_EXIT_TO_SCHEDULER or traceISR_EXIT.
void device_handler()
{
	traceISR_ENTER();
	BaseType_t woken = pdFALSE;
	std::uint32_t const value = sent_by_handler;
	xQueueSendFromISR(handler_queue, &value, &woken);
	portYIELD_FROM_ISR(woken);   // NOLINT(cppcoreguidelines-pro-type-cstyle-cast): the port's macro
}

void ping_task(void* /*unused*/)
{
	for (;;)
	{
		ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
		for (unsigned round = 0; round < rounds; ++round)
		{
			xTaskNotifyGive(pong);
			ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
		}
	}
}

void pong_task(void* /*unused*/)
{
	for (;;)
	{
		ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
		bounces = bounces + 1;
		xTaskNotifyGive(ping);
	}
}

void receiver_task(void* /*unused*/)
{
	for (;;)
	{
		std::uint32_t value = 0;
		if (xQueueReceive(handler_queue, &value, portMAX_DELAY) == pdTRUE)
			received = value;
	}
}

void low_task(void* /*unused*/)
{
	for (;;)
	{
		// A resume after a suspension ends the wait with no notification.
		if (ulTaskNotifyTake(pdTRUE, portMAX_DELAY) == 0)
			continue;
		if (low_job == job::hold_contested_mutex)
		{
			xSemaphoreTake(contested_mutex, portMAX_DELAY);
			xTaskNotifyGive(control);   // control preempts, and blocks on the mutex
			xSemaphoreGive(contested_mutex);
		}
	}
}

void ephemeral_task(void* /*unused*/)
{
	application_assert(__FILE__, __LINE__);   // deleted before it runs
}

TaskHandle_t create(TaskFunction_t entry, char const* name, UBaseType_t priority, StaticTask_t& tcb,
	std::array<StackType_t, stack_depth>& stack)
{
	return xTaskCreateStatic(entry, name, stack.size(), nullptr, priority, stack.data(), &tcb);
}

bool switches()
{
	xTaskNotifyGive(ping);   // ping preempts, and comes back once pong has answered each round
	return bounces == rounds;
}

bool handler_wakes()
{
	platform::pend();
	return received == sent_by_handler;
}

bool mutexes()
{
	bool pass = xSemaphoreTake(free_mutex, 0) == pdTRUE;
	pass = xSemaphoreGive(free_mutex) == pdTRUE && pass;

	pass = xSemaphoreTakeRecursive(recursive_mutex, 0) == pdTRUE && pass;
	pass = xSemaphoreTakeRecursive(recursive_mutex, 0) == pdTRUE && pass;
	pass = xSemaphoreGiveRecursive(recursive_mutex) == pdTRUE && pass;
	pass = xSemaphoreGiveRecursive(recursive_mutex) == pdTRUE && pass;

	low_job = job::hold_contested_mutex;
	xTaskNotifyGive(low);
	ulTaskNotifyTake(pdTRUE, portMAX_DELAY);   // low runs, takes the mutex and notifies back
	pass = xSemaphoreTake(contested_mutex, portMAX_DELAY) == pdTRUE && pass;
	pass = uxTaskPriorityGet(low) == 1 && pass;
	pass = xSemaphoreGive(contested_mutex) == pdTRUE && pass;
	low_job = job::nothing;
	return pass;
}

bool delays()
{
	vTaskDelay(1);
	TickType_t last = xTaskGetTickCount();
	return xTaskDelayUntil(&last, 1) == pdTRUE;
}

bool ready_priority()
{
	xTaskNotifyGive(low);   // ready, below control
	vTaskPrioritySet(low, 2);
	vTaskPrioritySet(low, 1);
	vTaskDelay(1);   // low runs, and waits again
	return uxTaskPriorityGet(low) == 1;
}

bool suspension()
{
	vTaskSuspend(low);
	vTaskResume(low);
	vTaskDelay(1);   // low runs, and waits again
	return true;
}

bool waits()
{
	std::uint32_t value = 0;
	bool pass = xQueuePeek(empty_queue, &value, 1) == pdFALSE;
	pass = xQueueReceive(empty_queue, &value, 1) == pdFALSE && pass;
	pass = xQueueSend(full_queue, &value, 0) == pdTRUE && pass;
	pass = xQueueSend(full_queue, &value, 1) == errQUEUE_FULL && pass;
	pass = ulTaskNotifyTake(pdTRUE, 1) == 0 && pass;
	pass = xTaskNotifyWait(0, 0, nullptr, 1) == pdFALSE && pass;
	pass = xEventGroupWaitBits(group, 1, pdTRUE, pdTRUE, 1) == 0 && pass;
	pass = xEventGroupSync(group, 2, 3, 1) == 2 && pass;
	std::array<std::uint8_t, stream_size> bytes{};
	pass = xStreamBufferReceive(stream, bytes.data(), bytes.size(), 1) == 0 && pass;
	pass = xStreamBufferSend(stream, bytes.data(), bytes.size(), 0) == bytes.size() && pass;
	pass = xStreamBufferSend(stream, bytes.data(), 1, 1) == 0 && pass;
	return pass;
}

bool creation()
{
	auto* const task = create(ephemeral_task, "ephemeral", 2, ephemeral_tcb, ephemeral_stack);
	vTaskDelete(task);
	return task != nullptr;
}

void control_task(void* /*unused*/)
{
	bool pass = switches();
	pass = handler_wakes() && pass;
	pass = mutexes() && pass;
	pass = delays() && pass;
	pass = ready_priority() && pass;
	pass = suspension() && pass;
	pass = waits() && pass;
	pass = creation() && pass;
	application_end(pass);
}

} // namespace application

void test_main()
{
	using namespace application;
	// The port's SysTick handler itself: the trace check names it.
#ifdef __ARM_ARCH_7EM__
	platform::prepare(&xPortSysTickHandler, &device_handler);
#else
	platform::prepare(&SysTick_Handler, &device_handler);
#endif

	handler_queue = xQueueCreateStatic(1, sizeof(std::uint32_t), handler_queue_items.data(), &handler_queue_storage);
	empty_queue = xQueueCreateStatic(1, sizeof(std::uint32_t), empty_queue_items.data(), &empty_queue_storage);
	full_queue = xQueueCreateStatic(1, sizeof(std::uint32_t), full_queue_items.data(), &full_queue_storage);
	free_mutex = xSemaphoreCreateMutexStatic(&free_mutex_storage);
	recursive_mutex = xSemaphoreCreateRecursiveMutexStatic(&recursive_mutex_storage);
	contested_mutex = xSemaphoreCreateMutexStatic(&contested_mutex_storage);
	group = xEventGroupCreateStatic(&group_storage);
	stream = xStreamBufferCreateStatic(stream_bytes.size(), 1, stream_bytes.data(), &stream_storage);

	control = create(control_task, "control", 3, control_tcb, control_stack);
	ping = create(ping_task, "ping", 4, ping_tcb, ping_stack);
	pong = create(pong_task, "pong", 4, pong_tcb, pong_stack);
	receiver = create(receiver_task, "receiver", 5, receiver_tcb, receiver_stack);
	low = create(low_task, "low", 1, low_tcb, low_stack);

	vTaskStartScheduler();
	application_assert(__FILE__, __LINE__);   // the scheduler does not return
}
