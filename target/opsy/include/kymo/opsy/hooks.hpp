/**
 * @file    hooks.hpp
 * @brief   The OpSy adapter: kymo::opsy::hooks turns what OpSy reports into
 *          Kymo events.
 *
 * Each event is named after the OpSy hook that emits it, and carries what
 * the hook gives: the addresses of the task, condition variable or mutex
 * involved, never their names. The host names a task from the ELF: the
 * symbol of a static task, or the entry function its control block holds.
 *
 * OpSy's own handlers, SysTick, the service call and PendSV, are family
 * kernel, which Kymo leaves off by default; the application's decorated
 * handlers are family interrupts.
 *
 * Not recorded: task_stopped (the next task_started or enter_idle says the
 * same), the lock hooks (masked sections come later), mutex_stored_for_task
 * and mutex_restored_for_task, condition_variable_start_waiting (task_wait
 * says it), task_name_changed. idle_wait is left to OpSy.
 *
 * Include it through OpSy: <opsy_hooks.hpp>, next to this directory, makes
 * these the program's hooks. A program that has hooks of its own defines
 * opsy::hooks as a class deriving from kymo::opsy::hooks, each of its hooks
 * calling the one it hides.
 *
 * Requires OpSy's root on the include path, for its config.hpp.
 */
#pragma once

#include <kymo/kymo.hpp>

#include <config.hpp>

#include <cstdint>

namespace opsy
{
class task_control_block;
class condition_variable;
class mutex;
enum class wake_reason : std::uint8_t;
} // namespace opsy

namespace kymo::opsy
{

using task_t = ::opsy::task_control_block const*;
using condition_variable_t = ::opsy::condition_variable const*;
using mutex_t = ::opsy::mutex const*;
using handler_t = void (*)();

// Scheduling

/// The scheduler starts.
struct starting
{
	static constexpr family kymo_family = family::scheduling;
	std::uint32_t core_clock; ///< in hertz
};

/// A task joins the active tasks.
struct task_added
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
	std::uint8_t priority; ///< opsy::task_priority, lower is higher
};

/// A task leaves the active tasks.
struct task_terminated
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
};

/// A task takes the core.
struct task_started
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
};

/// No task is ready: the core goes to the idle task.
struct enter_idle
{
	static constexpr family kymo_family = family::scheduling;
};

/// A task's priority changed, priority inheritance included.
struct task_priority_changed
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
	std::uint8_t priority;
};

// Causality

/// A task becomes ready.
struct task_ready
{
	static constexpr family kymo_family = family::causality;
	task_t task;
	::opsy::wake_reason reason;
};

/// A task goes to sleep.
struct task_sleep
{
	static constexpr family kymo_family = family::causality;
	task_t task{};
	::opsy::time_point until;
};

/// A task waits on a condition variable.
struct task_wait
{
	static constexpr family kymo_family = family::causality;
	task_t task;
	condition_variable_t condition;
};

/// A task waits on a condition variable, with a deadline.
struct task_wait_timeout
{
	static constexpr family kymo_family = family::causality;
	task_t task{};
	condition_variable_t condition{};
	::opsy::time_point until;
};

/// A condition variable is notified once; the context that emits it is the waker.
struct condition_variable_notify_one
{
	static constexpr family kymo_family = family::causality;
	condition_variable_t condition;
};

/// A condition variable is notified for all its waiters.
struct condition_variable_notify_all
{
	static constexpr family kymo_family = family::causality;
	condition_variable_t condition;
};

// Kernel: OpSy's own handlers

/// SysTick is entered.
struct enter_systick
{
	static constexpr family kymo_family = family::kernel;
};

/// SysTick returns.
struct exit_systick
{
	static constexpr family kymo_family = family::kernel;
	bool task_switch; ///< it asked for a context switch
};

/// The service call handler is entered.
struct enter_service_call
{
	static constexpr family kymo_family = family::kernel;
};

/// The service call handler returns.
struct exit_service_call
{
	static constexpr family kymo_family = family::kernel;
	bool task_switch; ///< it asked for a context switch
};

/// PendSV is entered, for a context switch.
struct enter_pend_sv
{
	static constexpr family kymo_family = family::kernel;
};

// Interrupts

/// A decorated interrupt handler is entered.
struct isr_enter
{
	static constexpr family kymo_family = family::interrupts;
	handler_t handler;
};

/// A decorated interrupt handler returns.
struct isr_exit
{
	static constexpr family kymo_family = family::interrupts;
	handler_t handler;
};

// Exclusion

/// A task takes a mutex.
struct mutex_taken
{
	static constexpr family kymo_family = family::exclusion;
	mutex_t mutex;
	task_t owner;
};

/// A task releases a mutex.
struct mutex_released
{
	static constexpr family kymo_family = family::exclusion;
	mutex_t mutex;
	task_t owner;
};

/// A task blocks on a mutex another task holds.
struct mutex_blocked
{
	static constexpr family kymo_family = family::exclusion;
	mutex_t mutex;
	task_t waiter;
};

/**
 * @brief The hooks OpSy calls, each emitting the event of its name.
 *
 * The hooks that read a task's priority are templates: OpSy declares
 * task_control_block, without defining it, where it includes them.
 */
struct hooks
{
	static void starting(auto& /*idle*/, std::uint32_t core_clock, auto&& /*foreach_task*/) { emit(kymo::opsy::starting{core_clock}); }

	static void task_added(auto& task) { emit(kymo::opsy::task_added{.task = &task, .priority = static_cast<std::uint8_t>(task.priority())}); }
	static void task_terminated(::opsy::task_control_block& task) { emit(kymo::opsy::task_terminated{&task}); }
	static void task_started(::opsy::task_control_block& task) { emit(kymo::opsy::task_started{&task}); }
	static void enter_idle() { emit(kymo::opsy::enter_idle{}); }
	static void task_priority_changed(auto& task)
	{
		emit(kymo::opsy::task_priority_changed{.task = &task, .priority = static_cast<std::uint8_t>(task.priority())});
	}

	static void task_ready(::opsy::task_control_block& task, ::opsy::wake_reason reason)
	{
		emit(kymo::opsy::task_ready{.task = &task, .reason = reason});
	}
	static void task_sleep(::opsy::task_control_block& task, ::opsy::time_point until)
	{
		emit(kymo::opsy::task_sleep{.task = &task, .until = until});
	}
	static void task_wait(::opsy::task_control_block& task, ::opsy::condition_variable& condition)
	{
		emit(kymo::opsy::task_wait{.task = &task, .condition = &condition});
	}
	static void task_wait_timeout(::opsy::task_control_block& task, ::opsy::condition_variable& condition,
		::opsy::time_point until)
	{
		emit(kymo::opsy::task_wait_timeout{.task = &task, .condition = &condition, .until = until});
	}
	static void condition_variable_notify_one(::opsy::condition_variable& condition)
	{
		emit(kymo::opsy::condition_variable_notify_one{&condition});
	}
	static void condition_variable_notify_all(::opsy::condition_variable& condition)
	{
		emit(kymo::opsy::condition_variable_notify_all{&condition});
	}

	static void enter_systick() { emit(kymo::opsy::enter_systick{}); }
	static void exit_systick(bool task_switch) { emit(kymo::opsy::exit_systick{task_switch}); }
	static void enter_service_call() { emit(kymo::opsy::enter_service_call{}); }
	static void exit_service_call(bool task_switch) { emit(kymo::opsy::exit_service_call{task_switch}); }
	static void enter_pend_sv() { emit(kymo::opsy::enter_pend_sv{}); }

	/** @brief The handler a vector table holds in place of @p Handler: the same, its entry and exit recorded */
	template<handler_t Handler>
	static constexpr auto decorate_isr()
	{
		return +[]
		{
			emit(isr_enter{Handler});
			Handler();
			emit(isr_exit{Handler});
		};
	}

	static void mutex_taken(::opsy::mutex& taken, ::opsy::task_control_block& owner)
	{
		emit(kymo::opsy::mutex_taken{.mutex = &taken, .owner = &owner});
	}
	static void mutex_released(::opsy::mutex& released, ::opsy::task_control_block& owner)
	{
		emit(kymo::opsy::mutex_released{.mutex = &released, .owner = &owner});
	}
	static void mutex_blocked(::opsy::mutex& blocking, ::opsy::task_control_block& waiter)
	{
		emit(kymo::opsy::mutex_blocked{.mutex = &blocking, .waiter = &waiter});
	}
};

} // namespace kymo::opsy
