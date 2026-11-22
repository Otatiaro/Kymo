/**
 * @file    opsy_hooks.hpp
 * @brief   OpSy's bench, traced: its markers, and every event the adapter
 *          emits, so that the bench counts the adapter's cost in each path.
 *
 * Put ahead of OpSy's tests/qemu/bench on the bench image's include path.
 * The bench namespace repeats what OpSy's bench hooks declare, for bench.cpp
 * to define. A scenario that begins at a marker begins before the event of
 * the same hook; every other marker follows its event, so that the event is
 * counted in the scenario either way.
 */
#pragma once

#include <kymo/opsy/hooks.hpp>

#include <cstdint>

namespace bench
{

enum class point : std::uint8_t
{
	notify,
	ready,
	isr_exit,
	pend_sv,
	started,
	count
};

void mark(point);
void systick_entered();

} // namespace bench

namespace opsy
{

class hooks : public kymo::opsy::hooks
{
	using traced = kymo::opsy::hooks;

public:
	static void condition_variable_notify_one(condition_variable& condition)
	{
		traced::condition_variable_notify_one(condition);
		bench::mark(bench::point::notify);
	}

	static void task_ready(task_control_block& task, wake_reason reason)
	{
		traced::task_ready(task, reason);
		bench::mark(bench::point::ready);
	}

	static void enter_pend_sv()
	{
		traced::enter_pend_sv();
		bench::mark(bench::point::pend_sv);
	}

	static void task_started(task_control_block& task)
	{
		traced::task_started(task);
		bench::mark(bench::point::started);
	}

	static void enter_systick()
	{
		bench::systick_entered();
		traced::enter_systick();
	}

	template<void (*Routine)()>
	static constexpr auto decorate_isr()
	{
		return +[]
		{
			traced::decorate_isr<Routine>()();
			bench::mark(bench::point::isr_exit);
		};
	}
};

} // namespace opsy
