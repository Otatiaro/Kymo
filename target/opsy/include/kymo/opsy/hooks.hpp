/**
 * @file    hooks.hpp
 * @brief   The OpSy adapter: kymo::opsy::hooks<Trace> turns what OpSy reports into
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
 * The events go through the application's tracer (kymo.hpp), which routes
 * each family to its channel. OpSy takes its hooks from the program's
 * <opsy_hooks.hpp>, which makes these the program's hooks:
 *
 *     #include <kymo/opsy/hooks.hpp>
 *     #include "trace.hpp" // the application's tracer
 *
 *     namespace opsy
 *     {
 *     /// A class, not an alias: OpSy's task_control_block names it as a friend.
 *     class hooks : public kymo::opsy::hooks<::trace>
 *     {};
 *     }
 *
 * A program that has hooks of its own declares them in that class, each
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

/// A task's effective priority moved: a donation through a mutex raised it
/// or ended, or its base priority was set.
struct task_priority_changed
{
	static constexpr family kymo_family = family::scheduling;
	task_t task;
	std::uint8_t priority; ///< the one it moved to, opsy::task_priority, lower is higher
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
 * @brief The hooks OpSy calls, each emitting the event of its name through
 *        @p Trace, the application's tracer.
 *
 * The hooks that read a task's priority are templates: OpSy declares
 * task_control_block, without defining it, where it includes them.
 */
template<typename Trace>
struct hooks
{
	static void starting(auto& /*idle*/, std::uint32_t core_clock, auto&& /*foreach_task*/) { Trace::emit(kymo::opsy::starting{core_clock}); }

	static void task_added(auto& task) { Trace::emit(kymo::opsy::task_added{.task = &task, .priority = static_cast<std::uint8_t>(task.current_priority())}); }
	static void task_terminated(::opsy::task_control_block& task) { Trace::emit(kymo::opsy::task_terminated{&task}); }
	static void task_started(::opsy::task_control_block& task) { Trace::emit(kymo::opsy::task_started{&task}); }
	static void enter_idle() { Trace::emit(kymo::opsy::enter_idle{}); }
	static void task_priority_changed(auto& task)
	{
		Trace::emit(kymo::opsy::task_priority_changed{.task = &task, .priority = static_cast<std::uint8_t>(task.current_priority())});
	}

	static void task_ready(::opsy::task_control_block& task, ::opsy::wake_reason reason)
	{
		Trace::emit(kymo::opsy::task_ready{.task = &task, .reason = reason});
	}
	static void task_sleep(::opsy::task_control_block& task, ::opsy::time_point until)
	{
		Trace::emit(kymo::opsy::task_sleep{.task = &task, .until = until});
	}
	static void task_wait(::opsy::task_control_block& task, ::opsy::condition_variable& condition)
	{
		Trace::emit(kymo::opsy::task_wait{.task = &task, .condition = &condition});
	}
	static void task_wait_timeout(::opsy::task_control_block& task, ::opsy::condition_variable& condition,
		::opsy::time_point until)
	{
		Trace::emit(kymo::opsy::task_wait_timeout{.task = &task, .condition = &condition, .until = until});
	}
	static void condition_variable_notify_one(::opsy::condition_variable& condition)
	{
		Trace::emit(kymo::opsy::condition_variable_notify_one{&condition});
	}
	static void condition_variable_notify_all(::opsy::condition_variable& condition)
	{
		Trace::emit(kymo::opsy::condition_variable_notify_all{&condition});
	}

	static void enter_systick() { Trace::emit(kymo::opsy::enter_systick{}); }
	static void exit_systick(bool task_switch) { Trace::emit(kymo::opsy::exit_systick{task_switch}); }
	static void enter_service_call() { Trace::emit(kymo::opsy::enter_service_call{}); }
	static void exit_service_call(bool task_switch) { Trace::emit(kymo::opsy::exit_service_call{task_switch}); }
	static void enter_pend_sv() { Trace::emit(kymo::opsy::enter_pend_sv{}); }

	/** @brief The handler a vector table holds in place of @p Handler: the same, its entry and exit recorded */
	template<handler_t Handler>
	static constexpr auto decorate_isr()
	{
		return +[]
		{
			Trace::emit(isr_enter{Handler});
			Handler();
			Trace::emit(isr_exit{Handler});
		};
	}

	static void mutex_taken(::opsy::mutex& taken, ::opsy::task_control_block& owner)
	{
		Trace::emit(kymo::opsy::mutex_taken{.mutex = &taken, .owner = &owner});
	}
	static void mutex_released(::opsy::mutex& released, ::opsy::task_control_block& owner)
	{
		Trace::emit(kymo::opsy::mutex_released{.mutex = &released, .owner = &owner});
	}
	static void mutex_blocked(::opsy::mutex& blocking, ::opsy::task_control_block& waiter)
	{
		Trace::emit(kymo::opsy::mutex_blocked{.mutex = &blocking, .waiter = &waiter});
	}
};

} // namespace kymo::opsy
