/**
 * @file    control.cpp
 * @brief   The control word: its value from reset, the filter by family, the
 *          freeze, and enable() and disable() interrupted at each of their
 *          instructions by a handler that changes another family.
 *
 * From reset the word enables every family but kernel. The startup code runs
 * no constructors, so the value from reset proves the word is
 * constant-initialised.
 */
#include "../qemu/preempt.hpp"
#include "../qemu/qemu.hpp"

#include "../test_trace.hpp"

#include <atomic>
#include <cstdint>

namespace
{

struct app_event
{
	std::uint32_t value;
};

struct switch_event
{
	static constexpr kymo::family kymo_family = kymo::family::scheduling;
	std::uint32_t task;
};

struct tick_event
{
	static constexpr kymo::family kymo_family = kymo::family::kernel;
	std::uint32_t tick;
};

static_assert(kymo::family_of<app_event> == kymo::family::application);
static_assert(kymo::family_of<switch_event> == kymo::family::scheduling);
static_assert(kymo::family_of<tick_event> == kymo::family::kernel);
static_assert(sizeof(switch_event) == 4, "kymo_family takes no room");

constexpr std::uint32_t all_but_kernel = 0x7FU;
static_assert(kymo::control_at_reset() == all_but_kernel, "every family but kernel from reset");

constexpr std::uint32_t application = kymo::bit(kymo::family::application);
constexpr std::uint32_t scheduling = kymo::bit(kymo::family::scheduling);
constexpr std::uint32_t max_calls = 10000;
constexpr std::uint32_t min_interrupted = 4;

std::uint32_t step;

[[noreturn]] void fail(char const* what)
{
	qemu::write("FAIL at step ");
	qemu::write_unsigned(step);
	qemu::write(": ");
	qemu::write(what);
	qemu::write("\n");
	qemu::finish(false);
}

void expect_word(std::uint32_t value, char const* what)
{
	++step;
	if (kymo::control.load(std::memory_order_relaxed) != value)
		fail(what);
}

/** @brief Checks that emitting @p event writes it when @p recorded, and nothing otherwise. */
template<typename Event>
void expect(Event const& event, bool recorded)
{
	++step;
	std::uint32_t const before = trace_ring.write_index.load(std::memory_order_relaxed);
	trace::emit(event);
	std::uint32_t const written = trace_ring.write_index.load(std::memory_order_relaxed) - before;
	if (written != (recorded ? 1U : 0U))
		fail(recorded ? "the event was not recorded" : "the event was recorded");
}

/// What the handler does to the word: enable or disable the scheduling family.
bool handler_enables;

/**
 * @brief Interrupts @p writer at each of its instructions in turn, the word
 *        holding @p start before each call; checks the word after it.
 *
 * The writer changes the application family, the handler the scheduling
 * family: both changes must hold, whatever instruction the handler lands on.
 */
void run(char const* name, void (*writer)(std::uint32_t), std::uint32_t start,
         std::uint32_t written, std::uint32_t both)
{
	std::uint32_t delay = 0;
	for (bool interrupted = true; interrupted;)
	{
		if (++delay > max_calls)
			fail("the writer outlasted every delay");
		kymo::control.store(start, std::memory_order_relaxed);
		interrupted = preempt::call(delay, writer, 0);
		expect_word(interrupted ? both : written, "a change to the word was lost");
	}
	qemu::write(name);
	qemu::write(": ");
	qemu::write_unsigned(delay - 1);
	qemu::write(" calls interrupted\n");
	if (delay - 1 < min_interrupted)
		fail("too few calls interrupted to mean anything");
}

} // namespace

extern "C" [[gnu::noinline]] void enable_application(std::uint32_t /*unused*/)
{
	kymo::enable(kymo::family::application);
}

extern "C" [[gnu::noinline]] void disable_application(std::uint32_t /*unused*/)
{
	kymo::disable(kymo::family::application);
}

void test_interrupt()
{
	if (handler_enables)
		kymo::enable(kymo::family::scheduling);
	else
		kymo::disable(kymo::family::scheduling);
}

void test_main()
{
	expect_word(all_but_kernel, "the control word is not control_at_reset() from reset");

	// From reset, every family but kernel is recorded.
	expect(app_event{1}, true);
	expect(switch_event{2}, true);
	expect(tick_event{3}, false);

	// The host clears the application family.
	kymo::control.store(all_but_kernel & ~application, std::memory_order_relaxed);
	expect(app_event{4}, false);
	expect(switch_event{5}, true);

	// The freeze stops everything.
	kymo::freeze();
	expect_word(0, "the freeze did not clear the word");
	expect(app_event{6}, false);
	expect(switch_event{7}, false);
	expect(tick_event{8}, false);

	// Enabling a family after a freeze records it again, and it alone.
	kymo::enable(kymo::family::scheduling);
	expect_word(scheduling, "enable() set other bits");
	expect(switch_event{9}, true);
	expect(app_event{10}, false);
	kymo::disable(kymo::family::scheduling);
	expect_word(0, "disable() left its bit");

	// enable() and disable() against a handler that changes another family.
	preempt::calibrate();
	handler_enables = false;
	run("enable()", enable_application, scheduling, scheduling | application, application);
	handler_enables = true;
	run("disable()", disable_application, application, 0, scheduling);

	qemu::finish(true);
}
