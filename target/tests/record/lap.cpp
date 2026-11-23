/**
 * @file    lap.cpp
 * @brief   Writers held for more than a lap of the ring while the host records
 *          (record.py --lap).
 *
 * Thread mode writes the events of record.cpp, one at a time, with a pause
 * after each. Every HOLD_EVERY events, SysTick lands on the write's first
 * instruction (preempt.hpp), then on each following one until the writer has
 * reserved its slots. Reserved and its first slot not yet published, the
 * writer is held: the interrupt emits `held` events, one slot each, for more
 * than a lap, so the writer resumes into slots newer events took. Already
 * published, the interrupt returns at once, and the event keeps the index it
 * was given. Every hold attempt lands in the window, however fast the host
 * reads.
 *
 * Its stale bytes stay in a newer event only until it writes its first
 * slot's header, with its own, older lap: a host reads them in that window
 * alone, and only for a newer event it has not read yet. So the last
 * SLOTS - 1 `held` events come with no pause, a burst the host has not read
 * when the writer resumes; and the interrupt that held it steps the resumed
 * writer an instruction or so at a time, until it changes a word of its slots, then
 * pauses it as long as each other `held` event: the host reads the window, and
 * must not take what it finds there for the newer event.
 *
 * Each `held` event carries its index and the ring's `lapped` count when it
 * was written: record.py sees from them that writers were lapped while it
 * recorded.
 *
 * KYMO_TEST_HOLD_PAUSE sets the loop rounds after each `held` event: long,
 * the host reads the held writer's slot unpublished before the lap; zero, the
 * lap falls between two of its reads.
 */
#include "events.hpp"

#include "../qemu/preempt.hpp"
#include "../qemu/qemu.hpp"

#include <cstdint>

struct held
{
	std::uint32_t index;
	std::uint32_t lapped;
};

namespace
{

using ring_type = decltype(kymo::trace_ring);

constexpr std::uint32_t slots = KYMO_RING_SLOTS;
constexpr std::uint32_t hold_every = 16;
constexpr std::uint32_t held_events = slots + slots / 4;
/// Loop rounds after each event of thread mode: the host keeps up.
constexpr std::uint32_t pause = 200000;
constexpr std::uint32_t hold_pause = KYMO_TEST_HOLD_PAUSE;

/// Slots of the largest event the writer writes (`four`).
constexpr std::uint32_t max_slots = 4;

struct slot_copy
{
	std::uint32_t header;
	std::uint32_t words[3];
};

std::uint32_t writer_index;   // the index the writer reserves
std::uint32_t writer_type;
bool volatile resumed;        // the next interrupt lands on the resumed writer
slot_copy before[max_slots];  // the writer's slots when it resumes
/// The shortest delay that outlasts the return from the interrupt: grown
/// each time a step lands on the instruction it stepped from.
std::uint32_t step = 1;
/// Added to the step on the way to the reservation, grown each time a step
/// lands back in a loop: an interrupt between LDREX and STREX fails the
/// reservation, which a step too short would interrupt again forever.
std::uint32_t stride;
std::uint32_t stepped_from;

void wait(std::uint32_t rounds)
{
	for (std::uint32_t volatile k = 0; k < rounds; k = k + 1)
	{
	}
}

} // namespace

extern "C" [[gnu::noinline]] void write_event(std::uint32_t n)
{
	emit_numbered(n, kymo::trace_ring.write_index.load(std::memory_order_relaxed));
}

/// Whether the resumed writer has changed a word of the slots it reserved.
bool overwritten()
{
	for (std::uint32_t k = 0; k < max_slots; ++k)
	{
		kymo::slot const& now = kymo::trace_ring.slots[(writer_index + k) % slots];
		if (now.header.load(std::memory_order_relaxed) != before[k].header
		    || now.words[0] != before[k].words[0] || now.words[1] != before[k].words[1]
		    || now.words[2] != before[k].words[2])
			return true;
	}
	return false;
}

/// Lands the next interrupt on an instruction past the one just interrupted.
void step_on()
{
	std::uint32_t const pc = preempt::interrupted();
	if (pc == stepped_from)
		++step;
	else if (pc < stepped_from && !resumed)
		++stride;
	stepped_from = pc;
	preempt::arm(step + stride);
}

void test_interrupt()
{
	if (resumed)
	{
		if (overwritten())
		{
			resumed = false;
			wait(hold_pause);
			return;
		}
		step_on();
		return;
	}
	if (kymo::trace_ring.write_index.load(std::memory_order_relaxed) == writer_index)
	{
		step_on();   // not reserved yet: on to the reservation
		return;
	}
	std::uint32_t const head = kymo::trace_ring.slots[writer_index % slots].header.load(std::memory_order_relaxed);
	if (head == ring_type::header(writer_type, writer_index, true))
		return;
	for (std::uint32_t i = 0; i < held_events; ++i)
	{
		kymo::emit(held{kymo::trace_ring.write_index.load(std::memory_order_relaxed),
		                kymo::trace_ring.lapped.load(std::memory_order_relaxed)});
		if (i < held_events - (slots - 1))
			wait(hold_pause);
	}
	for (std::uint32_t k = 0; k < max_slots; ++k)
	{
		kymo::slot const& now = kymo::trace_ring.slots[(writer_index + k) % slots];
		before[k].header = now.header.load(std::memory_order_relaxed);
		for (std::uint32_t w = 0; w < 3; ++w)
			before[k].words[w] = now.words[w];
	}
	resumed = true;
	stride = 0;
	stepped_from = preempt::interrupted();
	preempt::arm(step);
}

void test_main()
{
	preempt::calibrate();
	for (std::uint32_t n = 0;; ++n)
	{
		writer_index = kymo::trace_ring.write_index.load(std::memory_order_relaxed);
		writer_type = type_of(n);
		if (n % hold_every != hold_every - 1)
			write_event(n);
		else
		{
			stride = 0;
			stepped_from = 0;
			preempt::call(1, write_event, n);
		}
		wait(pause);
	}
}
