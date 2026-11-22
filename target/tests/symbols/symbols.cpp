/**
 * @file    symbols.cpp
 * @brief   Events whose fields point at functions and static objects, for
 *          the host to name them from the ELF (symbols.py).
 *
 * The objects take the shape of an RTOS's: a task derives from its stack
 * storage, then from its control block, so a control block pointer lands
 * inside the task at the stack's size. Next to them: a mutex that is a
 * member of a larger object, an interrupt handler, and addresses that are
 * in no symbol.
 */
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

#include <cstddef>
#include <cstdint>

namespace os
{

class task_control_block
{
public:
	std::uint32_t priority;
};

template <std::size_t Size>
class stack_storage
{
protected:
	alignas(8) std::byte stack[Size];
};

template <std::size_t Size>
class task : private stack_storage<Size>, public task_control_block
{
};

class mutex
{
public:
	task_control_block const* owner;
};

class condition_variable
{
public:
	std::uint32_t waiters;
};

} // namespace os

namespace app
{

struct peripheral
{
	std::uint32_t status;
	os::mutex lock;
};

os::task<256> first_task;
os::task<128> second_task;
os::mutex lock;
os::condition_variable ready;
peripheral uart;

std::uint32_t volatile interrupts;

void timer_handler()
{
	interrupts = interrupts + 1;
}

} // namespace app

struct task_started
{
	os::task_control_block const* task;
};

struct mutex_taken
{
	os::mutex* mutex;
	os::task_control_block const* owner;
};

struct notify
{
	os::condition_variable* condition;
};

struct isr_enter
{
	void (*handler)();
};

struct unnamed
{
	void const* outside;
	void const* null;
};

namespace
{

constexpr std::uint32_t pause = 200000;

} // namespace

void test_main()
{
	for (;;)
	{
		kymo::emit(task_started{&app::first_task});
		kymo::emit(task_started{&app::second_task});
		kymo::emit(mutex_taken{&app::lock, &app::first_task});
		kymo::emit(mutex_taken{&app::uart.lock, &app::second_task});
		kymo::emit(notify{&app::ready});
		kymo::emit(isr_enter{&app::timer_handler});
		kymo::emit(unnamed{reinterpret_cast<void const*>(0x40000000u), nullptr});
		for (std::uint32_t volatile k = 0; k < pause; k = k + 1)
		{
		}
	}
}
