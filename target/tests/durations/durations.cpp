/**
 * @file    durations.cpp
 * @brief   Durations, begun and ended in the thread and in an interrupt: how
 *          the host must pair them, and their texts (durations.py).
 *
 * The events go through a channel that stamps each with a clock of the
 * test's, whose gaps grow by 10 cycles at each event: QEMU has no cycle
 * counter, and each duration gets its own value. Run on its own, the image
 * reports the line the host must print for each event:
 *   event <index> <cycles> <type> <text, its fields written>
 * then each duration, each one still open at the end, and each event that
 * contradicts the pairing:
 *   duration <type> <index of its begin> <cycles>
 *   cut <type> <index of its begin>
 *   contradiction <index>
 * Recorded, the same image gives the capture the host reads.
 *
 * The SysTick exception, pended by the thread, stands for an interrupt: it
 * runs at once what the thread hands it.
 */
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

#include <atomic>
#include <cstdint>

struct moved
{
	std::uint8_t from;
	std::uint8_t to;
	static constexpr char kymo_text[] = "{from} -> {to}";   // NOLINT(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): the host reads it from the DWARF
};

struct step
{
	std::uint16_t number;
};

/// Waits for the interrupt.
struct waiting
{
};

struct batch
{
	std::uint32_t id;
	std::uint16_t count;
	static constexpr char kymo_text[] = "batch {id} of {count} in buffer {kymo_key}";   // NOLINT(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): the host reads it from the DWARF
};

/// Ended, never begun.
struct lonely
{
};

/// Begun, never ended.
struct endless
{
	std::int32_t code;
};

namespace
{

/// The ring the host drains, large enough to hold every event.
kymo::ring<256> durations_ring;

/// The test's clock, and the gap to the next event.
std::uint32_t now;
std::uint32_t gap;

/** @brief Writes to durations_ring, each event at the test's clock. */
struct clocked
{
	template<typename Event>
	static void write(std::uint32_t type, std::uint32_t /*timestamp*/, Event const& event)
	{
		durations_ring.write(type, now, event);
	}
};

clocked clocked_channel;

} // namespace

/** @brief Routes every family to the test's clocked channel. */
struct trace : kymo::tracer<trace>
{
	template<kymo::family F>
	static auto& channel(kymo::on<F> /*family*/)
	{
		return clocked_channel;
	}
};

namespace
{

/// Where an event goes: its index, and its cycles.
struct mark
{
	std::uint32_t index;
	std::uint32_t at;
};

/// Moves the clock on to the next event.
mark next()
{
	gap += 10;
	now += gap;
	return {durations_ring.write_index.load(std::memory_order_relaxed), now};
}

/// Reports the line of the event of type @p Event written at @p where.
template<typename Event>
void announce(mark where, char const* type, char const* text)
{
	qemu::write("event ");
	qemu::write_unsigned(where.index);
	if constexpr (kymo::slots_for<Event> > 1)
	{
		qemu::write("-");
		qemu::write_unsigned(where.index + kymo::slots_for<Event> - 1);
	}
	qemu::write(" ");
	qemu::write_unsigned(where.at);
	qemu::write(" ");
	qemu::write(type);
	if (*text != '\0')
	{
		qemu::write(" ");
		qemu::write(text);
	}
	qemu::write("\n");
}

/// Writes an event of type @p Event with @p write, and reports it.
template<typename Event, typename Write>
mark traced(Write write, char const* type, char const* text)
{
	mark const where = next();
	write();
	announce<Event>(where, type, text);
	return where;
}

void duration(char const* type, mark begin, mark end)
{
	qemu::write("duration ");
	qemu::write(type);
	qemu::write(" ");
	qemu::write_unsigned(begin.index);
	qemu::write(" ");
	qemu::write_unsigned(end.at - begin.at);
	qemu::write("\n");
}

void cut(char const* type, mark begin)
{
	qemu::write("cut ");
	qemu::write(type);
	qemu::write(" ");
	qemu::write_unsigned(begin.index);
	qemu::write("\n");
}

void contradiction(mark where)
{
	qemu::write("contradiction ");
	qemu::write_unsigned(where.index);
	qemu::write("\n");
}

/// What the interrupt runs, and where its event went.
void (*handed)() = nullptr;
mark in_interrupt;

std::uint32_t volatile& reg(std::uintptr_t address)
{
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): a register at a fixed address
	return *reinterpret_cast<std::uint32_t volatile*>(address);
}

constexpr std::uintptr_t icsr = 0xE000ED04U;
constexpr std::uint32_t icsr_pendstset = 1U << 26;

/// Runs @p work in the interrupt, and returns where its event went.
mark interrupt(void (*work)())
{
	handed = work;
	reg(icsr) = icsr_pendstset;
	asm volatile("dsb\n\tisb" ::: "memory");
	return in_interrupt;
}

using begin_moved = kymo::begin_of<moved>;
using end_moved = kymo::end_of<moved>;
using begin_batch = kymo::keyed_begin_of<batch>;
using end_batch = kymo::keyed_end_of<batch>;

} // namespace

extern "C" void SysTick_Handler()
{
	handed();
}

void test_main()
{
	// Nested durations of one type, the inner one a scope; a step begun
	// within the outer one ends after it: an end closes its own type.
	mark const outer = traced<begin_moved>([] { trace::begin(moved{1, 2}); }, "kymo::begin_of<moved>", "1 -> 2");
	mark inner{};
	mark inner_end{};
	{
		inner = next();
		auto const open = trace::scope(moved{3, 4});
		announce<begin_moved>(inner, "kymo::begin_of<moved>", "3 -> 4");
		inner_end = next();
	}
	announce<end_moved>(inner_end, "kymo::end_of<moved>", "");
	mark const step_begin = traced<kymo::begin_of<step>>([] { trace::begin(step{9}); }, "kymo::begin_of<step>", "number=9");
	mark const outer_end = traced<end_moved>([] { trace::end<moved>(); }, "kymo::end_of<moved>", "");
	mark const step_end = traced<kymo::end_of<step>>([] { trace::end<step>(); }, "kymo::end_of<step>", "");
	duration("moved", outer, outer_end);
	duration("moved", inner, inner_end);
	duration("step", step_begin, step_end);

	// Begun in the thread, ended in the interrupt.
	mark const wait = traced<kymo::begin_of<waiting>>([] { trace::begin(waiting{}); }, "kymo::begin_of<waiting>", "");
	mark const woken = interrupt([] {
		in_interrupt = traced<kymo::end_of<waiting>>([] { trace::end<waiting>(); }, "kymo::end_of<waiting>", "");
	});
	duration("waiting", wait, woken);

	// A double buffer: the interrupt fills one buffer while the thread works
	// through the other. Each end closes the batch of its buffer, where a
	// stack would close the last one begun.
	mark const first = traced<begin_batch>([] { trace::begin(batch{1, 8}, 0); }, "kymo::keyed_begin_of<batch>", "batch 1 of 8 in buffer 0");
	mark const second = interrupt([] {
		in_interrupt = traced<begin_batch>([] { trace::begin(batch{2, 8}, 1); }, "kymo::keyed_begin_of<batch>", "batch 2 of 8 in buffer 1");
	});
	mark const first_end = traced<end_batch>([] { trace::end<batch>(0); }, "kymo::keyed_end_of<batch>", "kymo_key=0");
	mark const third = interrupt([] {
		in_interrupt = traced<begin_batch>([] { trace::begin(batch{3, 5}, 0); }, "kymo::keyed_begin_of<batch>", "batch 3 of 5 in buffer 0");
	});
	mark const second_end = traced<end_batch>([] { trace::end<batch>(1); }, "kymo::keyed_end_of<batch>", "kymo_key=1");
	mark const third_end = traced<end_batch>([] { trace::end<batch>(0); }, "kymo::keyed_end_of<batch>", "kymo_key=0");
	duration("batch", first, first_end);
	duration("batch", second, second_end);
	duration("batch", third, third_end);

	// What contradicts the pairing, and a duration still open at the end.
	contradiction(traced<kymo::end_of<lonely>>([] { trace::end<lonely>(); }, "kymo::end_of<lonely>", ""));
	contradiction(traced<end_batch>([] { trace::end<batch>(7); }, "kymo::keyed_end_of<batch>", "kymo_key=7"));
	cut("endless", traced<kymo::begin_of<endless>>([] { trace::begin(endless{-1}); }, "kymo::begin_of<endless>", "code=-1"));
	traced<step>([] { trace::emit(step{1}); }, "step", "number=1");

	qemu::finish(true);
}
