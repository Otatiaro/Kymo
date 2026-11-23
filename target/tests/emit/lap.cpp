/**
 * @file    lap.cpp
 * @brief   A writer lapped by the ring counts itself in `lapped`: once when it
 *          wrote into slots no longer its own, never when the ring did not
 *          lap it.
 *
 * A writer emits a three-slot event from thread mode. An interrupt lands on
 * one of its instructions and emits a full ring of one-slot events, so the
 * slots the writer reserved, if it had, are reserved again by newer events.
 * The interrupt then keeps a copy of the ring. When the call returns:
 *  - interrupted before its reservation, the writer was not lapped: `lapped`
 *    is unchanged;
 *  - interrupted after its reservation and before it published its first
 *    slot, the writer went on writing into newer events' slots: the ring
 *    differs from the copy, and `lapped` went up by one;
 *  - interrupted after it published its first slot, it wrote nothing stale:
 *    the ring equals the copy, and `lapped` went up by one when the interrupt
 *    came before the writer read the write index again, by zero after.
 *
 * The interrupt sweeps every instruction of the writer (preempt.hpp). The
 * write index starts short of 2^32, so the lap test wraps too.
 */
#include "../qemu/preempt.hpp"
#include "../qemu/qemu.hpp"

#include "../test_trace.hpp"

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>

namespace
{

struct lap_event
{
	std::uint32_t seq;
	std::uint32_t check;
};

struct large_event
{
	std::uint32_t seq;
	std::array<std::uint32_t, 7> words;
};

static_assert(kymo::slots_for<lap_event> == 1);
static_assert(kymo::slots_for<large_event> == 3);

constexpr std::uint32_t slots = ring_slots;
constexpr std::uint32_t max_delay = 100000;

using ring_type = decltype(trace_ring);

/// Where the writer stood when the interrupt landed.
enum class landed : std::uint8_t
{
	none,        ///< the call ended first
	before,      ///< before the writer reserved its slots
	unpublished, ///< reserved, its first slot not yet published
	published,   ///< its first slot published
};

std::uint32_t writer_index;   // the index the writer reserves
landed volatile where;
std::uint32_t lap_seq;
using slot_bytes = std::array<unsigned char, sizeof(trace_ring.slots)>;
slot_bytes copy;

[[noreturn]] void fail(char const* what, std::uint32_t delay)
{
	qemu::write("FAIL with delay ");
	qemu::write_unsigned(delay);
	qemu::write(": ");
	qemu::write(what);
	qemu::write("\n");
	qemu::finish(false);
}

/** @brief Whether the ring's slots hold what the interrupt copied. */
bool same_as_copy()
{
	slot_bytes live{};
	std::memcpy(live.data(), static_cast<void const*>(trace_ring.slots.data()), live.size());
	return live == copy;
}

} // namespace

extern "C" [[gnu::noinline]] void write_large(std::uint32_t seq)
{
	large_event event{};
	event.seq = seq;
	for (std::uint32_t w = 0; w < event.words.size(); ++w)
		event.words[w] = seq + w;
	trace::emit(event);
}

void test_interrupt()
{
	std::uint32_t const index = trace_ring.write_index.load(std::memory_order_relaxed);
	std::uint32_t const head = trace_ring.slots[writer_index % slots].header.load(std::memory_order_relaxed);
	if (index == writer_index)
		where = landed::before;
	else if (head == ring_type::header(kymo::type_id<large_event>(), writer_index, true))
		where = landed::published;
	else
		where = landed::unpublished;
	for (std::uint32_t i = 0; i < slots; ++i)
	{
		trace::emit(lap_event{lap_seq, ~lap_seq});
		++lap_seq;
	}
	std::memcpy(copy.data(), static_cast<void const*>(trace_ring.slots.data()), copy.size());
}

void test_main()
{
	preempt::calibrate();
	trace_ring.write_index.store(0U - (3 * slots), std::memory_order_relaxed);

	std::array<std::uint32_t, 4> seen{};
	std::uint32_t published_counted = 0;
	for (std::uint32_t delay = 1;; ++delay)
	{
		if (delay > max_delay)
			fail("the writer outlasted every delay", delay);
		writer_index = trace_ring.write_index.load(std::memory_order_relaxed);
		std::uint32_t const lapped = trace_ring.lapped.load(std::memory_order_relaxed);
		where = landed::none;
		preempt::call(delay, write_large, delay);
		if (where == landed::none)
			break;
		std::uint32_t const counted = trace_ring.lapped.load(std::memory_order_relaxed) - lapped;
		bool const stale = !same_as_copy();
		switch (where)
		{
		case landed::before:
			if (counted != 0)
				fail("a writer the ring did not lap was counted", delay);
			break;
		case landed::unpublished:
			if (!stale)
				fail("a lapped writer wrote nothing after the lap", delay);
			if (counted != 1)
				fail("a writer that wrote into newer events' slots was not counted once", delay);
			break;
		case landed::published:
			if (stale)
				fail("a writer wrote after publishing its first slot", delay);
			if (counted > 1)
				fail("a writer was counted more than once", delay);
			published_counted += counted;
			break;
		case landed::none:
			break;
		}
		++seen[static_cast<std::size_t>(where)];
	}
	if (seen[1] == 0 || seen[2] == 0 || seen[3] == 0)
		fail("an interrupt position was never reached", 0);
	qemu::write("interrupted before the reservation: ");
	qemu::write_unsigned(seen[1]);
	qemu::write(", before publishing: ");
	qemu::write_unsigned(seen[2]);
	qemu::write(", after publishing: ");
	qemu::write_unsigned(seen[3]);
	qemu::write(" (counted ");
	qemu::write_unsigned(published_counted);
	qemu::write(")\n");
	qemu::finish(true);
}
