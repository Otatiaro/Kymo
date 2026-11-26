/**
 * @file    application.cpp
 * @brief   The Zephyr test application: the main thread, control, runs one
 *          scenario after another, each taking the kernel through trace
 *          points the adapter records, then exits through qemu::exit with its
 *          verdict. trace/trace.py records it and checks the recording.
 *
 * Priorities, lower is higher: receiver 1, ping and pong 2, control 3,
 * ephemeral and writer 4, low 5, idle last.
 *
 * Scenarios, in order:
 * 1. switches: ping and pong hand the core to each other three times;
 * 2. a handler wakes the receiver through a message queue;
 * 3. a free mutex, and a mutex locked twice and unlocked twice, by control;
 *    then a mutex low holds and control blocks on: low inherits control's
 *    priority, and drops back when it unlocks the mutex, which control takes;
 * 4. a sleep of a tick;
 * 5. the priority of a ready thread changed, twice;
 * 6. control blocks for a tick on each kind of wait: semaphore, message queue
 *    get and put, event, pipe read and write;
 * 7. control gets from a full message queue a writer blocks on: the get hands
 *    the slot to the writer, and does not block;
 * 8. a thread created, named and aborted before it runs.
 */
#include "qemu.hpp"

#include <zephyr/irq.h>
#include <zephyr/kernel.h>

#include <cmsis_core.h>

#include <array>
#include <cerrno>
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

constexpr std::size_t stack_size = 1024;
/// The rounds of the switch scenario, each a switch to pong and one back.
constexpr unsigned rounds = 3;
constexpr std::uint32_t sent_by_handler = 42;
/// The interrupt the handler serves: the last of the part, which no driver
/// uses.
constexpr unsigned device_irq = CONFIG_NUM_IRQS - 1;
constexpr int low_priority = 5;
constexpr std::size_t pipe_size = 4;

/// What low does when control signals it.
enum class job : std::uint8_t
{
	nothing,
	hold_contested_mutex,
};
job volatile low_job = job::nothing;

unsigned volatile bounces = 0;
std::uint32_t volatile received = 0;

K_SEM_DEFINE(ping_sem, 0, 1);
K_SEM_DEFINE(pong_sem, 0, 1);
K_SEM_DEFINE(low_sem, 0, 1);
K_SEM_DEFINE(control_sem, 0, 1);
K_SEM_DEFINE(writer_sem, 0, 1);
K_SEM_DEFINE(empty_sem, 0, 1);

// NOLINTBEGIN(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays,cppcoreguidelines-pro-bounds-array-to-pointer-decay,cppcoreguidelines-pro-bounds-pointer-arithmetic,misc-use-anonymous-namespace): the kernel's definition macro
K_MSGQ_DEFINE(handler_msgq, sizeof(std::uint32_t), 1, 4);
K_MSGQ_DEFINE(empty_msgq, sizeof(std::uint32_t), 1, 4);
K_MSGQ_DEFINE(full_msgq, sizeof(std::uint32_t), 1, 4);
K_MSGQ_DEFINE(handoff_msgq, sizeof(std::uint32_t), 1, 4);
// NOLINTEND(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays,cppcoreguidelines-pro-bounds-array-to-pointer-decay,cppcoreguidelines-pro-bounds-pointer-arithmetic,misc-use-anonymous-namespace)

K_MUTEX_DEFINE(free_mutex);
K_MUTEX_DEFINE(recursive_mutex);
K_MUTEX_DEFINE(contested_mutex);

K_EVENT_DEFINE(event);

std::array<std::uint8_t, pipe_size> pipe_bytes;
k_pipe pipe;

k_thread ephemeral;
K_THREAD_STACK_DEFINE(ephemeral_stack, stack_size);   // NOLINT(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): the kernel's definition macro

/** @brief Connected through the kernel's wrapper: the trace names it from the
 *  software ISR table. */
void device_handler(void const* /*unused*/)
{
	std::uint32_t const value = sent_by_handler;
	k_msgq_put(&handler_msgq, &value, K_NO_WAIT);
}

void ping_entry(void* /*unused*/, void* /*unused*/, void* /*unused*/)
{
	for (;;)
	{
		k_sem_take(&ping_sem, K_FOREVER);
		for (unsigned round = 0; round < rounds; ++round)
		{
			k_sem_give(&pong_sem);
			k_sem_take(&ping_sem, K_FOREVER);
		}
	}
}

void pong_entry(void* /*unused*/, void* /*unused*/, void* /*unused*/)
{
	for (;;)
	{
		k_sem_take(&pong_sem, K_FOREVER);
		bounces = bounces + 1;
		k_sem_give(&ping_sem);
	}
}

void receiver_entry(void* /*unused*/, void* /*unused*/, void* /*unused*/)
{
	for (;;)
	{
		std::uint32_t value = 0;
		if (k_msgq_get(&handler_msgq, &value, K_FOREVER) == 0)
			received = value;
	}
}

void low_entry(void* /*unused*/, void* /*unused*/, void* /*unused*/)
{
	for (;;)
	{
		k_sem_take(&low_sem, K_FOREVER);
		if (low_job == job::hold_contested_mutex)
		{
			k_mutex_lock(&contested_mutex, K_FOREVER);
			k_sem_give(&control_sem);   // control preempts, and blocks on the mutex
			k_mutex_unlock(&contested_mutex);
		}
	}
}

void writer_entry(void* /*unused*/, void* /*unused*/, void* /*unused*/)
{
	for (;;)
	{
		k_sem_take(&writer_sem, K_FOREVER);
		std::uint32_t const value = 0;
		k_msgq_put(&handoff_msgq, &value, K_FOREVER);
	}
}

void ephemeral_entry(void* /*unused*/, void* /*unused*/, void* /*unused*/)
{
	application_assert(__FILE__, __LINE__);   // aborted before it runs
}

} // namespace application

// The threads' objects, named by the host from the ELF: _k_thread_obj_<name>.
// NOLINTBEGIN(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays,cppcoreguidelines-pro-bounds-array-to-pointer-decay,readability-math-missing-parentheses,bugprone-branch-clone,misc-misplaced-const): the kernel's definition macro
K_THREAD_DEFINE(receiver, application::stack_size, application::receiver_entry, nullptr, nullptr, nullptr, 1, 0, 0);
K_THREAD_DEFINE(ping, application::stack_size, application::ping_entry, nullptr, nullptr, nullptr, 2, 0, 0);
K_THREAD_DEFINE(pong, application::stack_size, application::pong_entry, nullptr, nullptr, nullptr, 2, 0, 0);
K_THREAD_DEFINE(writer, application::stack_size, application::writer_entry, nullptr, nullptr, nullptr, 4, 0, 0);
K_THREAD_DEFINE(low, application::stack_size, application::low_entry, nullptr, nullptr, nullptr,
	application::low_priority, 0, 0);
// NOLINTEND(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays,cppcoreguidelines-pro-bounds-array-to-pointer-decay,readability-math-missing-parentheses,bugprone-branch-clone,misc-misplaced-const)

namespace application
{

bool switches()
{
	k_sem_give(&ping_sem);   // ping preempts, and comes back once pong has answered each round
	return bounces == rounds;
}

bool handler_wakes()
{
	NVIC_SetPendingIRQ(static_cast<IRQn_Type>(device_irq));   // NOLINT(clang-analyzer-optin.core.EnumCastOutOfRange): an interrupt the part's enum does not name
	__DSB();
	__ISB();
	return received == sent_by_handler;
}

bool mutexes()
{
	bool pass = k_mutex_lock(&free_mutex, K_NO_WAIT) == 0;
	pass = k_mutex_unlock(&free_mutex) == 0 && pass;

	pass = k_mutex_lock(&recursive_mutex, K_NO_WAIT) == 0 && pass;
	pass = k_mutex_lock(&recursive_mutex, K_NO_WAIT) == 0 && pass;
	pass = k_mutex_unlock(&recursive_mutex) == 0 && pass;
	pass = k_mutex_unlock(&recursive_mutex) == 0 && pass;

	low_job = job::hold_contested_mutex;
	k_sem_give(&low_sem);
	k_sem_take(&control_sem, K_FOREVER);   // low runs, locks the mutex and signals back
	pass = k_mutex_lock(&contested_mutex, K_FOREVER) == 0 && pass;
	pass = k_thread_priority_get(low) == low_priority && pass;
	pass = k_mutex_unlock(&contested_mutex) == 0 && pass;
	low_job = job::nothing;
	return pass;
}

bool sleeps()
{
	return k_sleep(K_TICKS(1)) == 0;
}

bool ready_priority()
{
	k_sem_give(&low_sem);   // ready, below control
	k_thread_priority_set(low, low_priority - 1);
	k_thread_priority_set(low, low_priority);
	k_sleep(K_TICKS(1));   // low runs, and waits again
	return k_thread_priority_get(low) == low_priority;
}

bool waits()
{
	std::uint32_t value = 0;
	bool pass = k_sem_take(&empty_sem, K_TICKS(1)) == -EAGAIN;
	pass = k_msgq_get(&empty_msgq, &value, K_TICKS(1)) == -EAGAIN && pass;
	pass = k_msgq_put(&full_msgq, &value, K_NO_WAIT) == 0 && pass;
	pass = k_msgq_put(&full_msgq, &value, K_TICKS(1)) == -EAGAIN && pass;
	pass = k_event_wait(&event, 1, false, K_TICKS(1)) == 0 && pass;
	std::array<std::uint8_t, pipe_size> bytes{};
	pass = k_pipe_read(&pipe, bytes.data(), bytes.size(), K_TICKS(1)) == -EAGAIN && pass;
	pass = k_pipe_write(&pipe, bytes.data(), bytes.size(), K_NO_WAIT) == static_cast<int>(bytes.size()) && pass;
	pass = k_pipe_write(&pipe, bytes.data(), 1, K_TICKS(1)) == -EAGAIN && pass;
	return pass;
}

bool handoff()
{
	std::uint32_t value = 0;
	bool pass = k_msgq_put(&handoff_msgq, &value, K_NO_WAIT) == 0;
	k_sem_give(&writer_sem);
	k_sleep(K_TICKS(1));   // the writer runs, and blocks on the full queue
	pass = k_msgq_get(&handoff_msgq, &value, K_NO_WAIT) == 0 && pass;   // hands the slot to the writer
	pass = k_msgq_get(&handoff_msgq, &value, K_NO_WAIT) == 0 && pass;   // the writer's message
	return pass;
}

bool creation()
{
	// NOLINTNEXTLINE(cppcoreguidelines-pro-bounds-array-to-pointer-decay): the kernel takes the stack it defined as its first element
	k_thread* const thread = k_thread_create(&ephemeral, ephemeral_stack, K_THREAD_STACK_SIZEOF(ephemeral_stack),
		ephemeral_entry, nullptr, nullptr, nullptr, 4, 0, K_FOREVER);
	bool const pass = k_thread_name_set(thread, "ephemeral") == 0;
	k_thread_abort(thread);
	return pass;
}

} // namespace application

int main()
{
	using namespace application;
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-cstyle-cast,misc-redundant-expression,bugprone-signed-bitwise,misc-const-correctness): the kernel's macro
	IRQ_CONNECT(device_irq, 1, device_handler, nullptr, 0);
	irq_enable(device_irq);
	k_pipe_init(&pipe, pipe_bytes.data(), pipe_bytes.size());

	bool pass = switches();
	pass = handler_wakes() && pass;
	pass = mutexes() && pass;
	pass = sleeps() && pass;
	pass = ready_priority() && pass;
	pass = waits() && pass;
	pass = handoff() && pass;
	pass = creation() && pass;
	application_end(pass);
	return 0;
}
