/**
 * @file    preemption.cpp
 * @brief   No slot lost, none written twice, when an interrupt that emits
 *          lands on any instruction of an emit.
 *
 * A writer emits from thread mode, and SysTick emits too. Each call to a
 * writer is interrupted at one instruction, the next call at the next one,
 * until a call ends before the interrupt: every instruction of the writer has
 * then been interrupted (preempt.hpp, which checks that none is skipped).
 * After each call the image walks the slots written since the previous one
 * and checks that they hold exactly the writer's event and the handler's,
 * each whole, with nothing left over. The handler also reads the ring before
 * it writes, as the host would in the middle of a write: every event whose
 * head slot is published must be whole.
 *
 * Two writers: an event of one slot, and one of three; the handler's takes
 * two. The ring is small (KYMO_RING_SLOTS) so that events keep wrapping into
 * the guard zone, and its index starts short of 2^32 so that it wraps too.
 */
#include "../qemu/preempt.hpp"
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>

namespace
{

struct small_event
{
	std::uint32_t seq;
	std::uint32_t check;
};

struct large_event
{
	std::uint32_t seq;
	std::array<std::uint32_t, 7> words;
};

struct isr_event
{
	std::uint32_t seq;
	std::array<std::uint32_t, 4> words;
};

/// Room for the largest event.
using event_bytes = std::array<unsigned char, sizeof(large_event)>;

static_assert(kymo::slots_for<small_event> == 1);
static_assert(kymo::slots_for<large_event> == 3);
static_assert(kymo::slots_for<isr_event> == 2);

constexpr std::uint32_t pattern(std::uint32_t seq, std::uint32_t word)
{
	return (seq * 0x9E3779B1U) ^ (word * 0x85EBCA6BU) ^ 0xA5A5A5A5U;
}

constexpr std::uint32_t max_calls = 100000;
constexpr std::uint32_t min_interrupted = 8;

std::uint32_t isr_written;    // events the handler has emitted
std::uint32_t isr_checked;    // of those, the ones found in the ring
std::uint32_t checked_index;  // where the next check starts

using ring_type = decltype(kymo::trace_ring);
constexpr std::uint32_t slot_mask = KYMO_RING_SLOTS - 1;

[[noreturn]] void fail(char const* what, std::uint32_t index)
{
	qemu::write("FAIL at index ");
	qemu::write_unsigned(index);
	qemu::write(": ");
	qemu::write(what);
	qemu::write("\n");
	qemu::finish(false);
}

/** @brief Reads the event at @p index into @p out; returns its slot count. */
std::uint32_t read_event(std::uint32_t index, std::uint16_t& type, event_bytes& out)
{
	// An event's slots follow each other, into the guard zone past the end.
	std::size_t const position = index & slot_mask;
	kymo::slot const& first = kymo::trace_ring.slots[position];
	std::uint32_t const head = first.header.load(std::memory_order_acquire);
	type = static_cast<std::uint16_t>(head & 0xffffU);
	if (head != ring_type::header(type, index, true))
		fail("no complete head slot", index);

	std::size_t size = 0;
	if (type == kymo::type_id<small_event>())
		size = sizeof(small_event);
	else if (type == kymo::type_id<large_event>())
		size = sizeof(large_event);
	else if (type == kymo::type_id<isr_event>())
		size = sizeof(isr_event);
	else
		fail("unknown event type", index);

	std::uint32_t const count = size <= kymo::head_payload
		? 1
		: 1 + ((size - kymo::head_payload + kymo::tail_payload - 1) / kymo::tail_payload);
	std::memcpy(out.data(), &first.words[1], size < kymo::head_payload ? size : kymo::head_payload);
	for (std::uint32_t i = 1; i < count; ++i)
	{
		kymo::slot const& tail = kymo::trace_ring.slots[position + i];
		if (tail.header.load(std::memory_order_acquire) != ring_type::header(type, index, false))
			fail("no complete continuation slot", index + i);
		std::size_t const offset = kymo::head_payload + ((i - 1) * kymo::tail_payload);
		std::size_t const left = size - offset;
		std::memcpy(&out[offset], tail.words.data(), left < kymo::tail_payload ? left : kymo::tail_payload);
	}
	return count;
}

/**
 * @brief Reads, as the host would, every event published since the last
 *        check: an event whose head slot is published must be whole.
 *
 * Runs in the handler, so in the middle of a write: the walk stops at the
 * first head slot not yet published, which is the interrupted writer's.
 */
void read_published()
{
	std::uint32_t const end = kymo::trace_ring.write_index.load(std::memory_order_acquire);
	std::uint32_t index = checked_index;
	while (index != end)
	{
		std::uint32_t const head = kymo::trace_ring.slots[index & slot_mask].header.load(std::memory_order_acquire);
		if (head != ring_type::header(static_cast<std::uint16_t>(head & 0xffffU), index, true))
			return;
		event_bytes bytes{};
		std::uint16_t type = 0;
		index += read_event(index, type, bytes);
	}
}

/** @brief Checks the writer's event, of @p type, read at @p index. */
void check_writer(std::uint16_t type, std::uint32_t writer_seq, event_bytes const& bytes, std::uint32_t index)
{
	std::uint32_t seq = 0;
	std::memcpy(&seq, bytes.data(), sizeof seq);
	if (seq != writer_seq)
		fail("the writer's event has the wrong sequence", index);
	if (type == kymo::type_id<small_event>())
	{
		small_event event{};
		std::memcpy(&event, bytes.data(), sizeof event);
		if (event.check != ~writer_seq)
			fail("the small event's payload is wrong", index);
		return;
	}
	large_event event{};
	std::memcpy(&event, bytes.data(), sizeof event);
	for (std::uint32_t w = 0; w < event.words.size(); ++w)
		if (event.words[w] != pattern(writer_seq, w))
			fail("the large event's payload is wrong", index);
}

/** @brief Checks the handler's event read at @p index, the next one it wrote. */
void check_isr(event_bytes const& bytes, std::uint32_t index)
{
	isr_event event{};
	std::memcpy(&event, bytes.data(), sizeof event);
	if (event.seq != isr_checked)
		fail("the handler's event has the wrong sequence", index);
	for (std::uint32_t w = 0; w < event.words.size(); ++w)
		if (event.words[w] != pattern(~event.seq, w))
			fail("the handler's event's payload is wrong", index);
	++isr_checked;
}

/** @brief Checks the slots written since the last check. @return whether the handler wrote in them. */
bool check(std::uint16_t writer_type, std::uint32_t writer_seq)
{
	std::uint32_t const end = kymo::trace_ring.write_index.load(std::memory_order_acquire);
	bool writer_found = false;
	bool interrupted = false;
	std::uint32_t index = checked_index;
	while (index != end)
	{
		if (end - index > KYMO_RING_SLOTS)
			fail("more slots written than the ring holds", index);
		event_bytes bytes{};
		std::uint16_t type = 0;
		std::uint32_t const count = read_event(index, type, bytes);
		if (type == writer_type)
		{
			if (writer_found)
				fail("the writer's event is there twice", index);
			writer_found = true;
			check_writer(type, writer_seq, bytes, index);
		}
		else if (type == kymo::type_id<isr_event>())
		{
			check_isr(bytes, index);
			interrupted = true;
		}
		else
		{
			fail("an event of the other writer", index);
		}
		index += count;
	}
	if (!writer_found)
		fail("the writer's event is missing", index);
	if (isr_checked != isr_written)
		fail("a handler event is missing", index);
	checked_index = end;
	return interrupted;
}

/** @brief Interrupts @p writer at each of its instructions in turn. */
void run(char const* name, std::uint16_t type, void (*writer)(std::uint32_t), std::uint32_t& seq)
{
	std::uint32_t delay = 0;
	for (bool interrupted = true; interrupted; ++seq)
	{
		if (++delay > max_calls)
			fail("the writer outlasted every delay", checked_index);
		interrupted = preempt::call(delay, writer, seq);
		if (check(type, seq) != interrupted)
			fail("the handler's event and the interrupt disagree", checked_index);
	}
	qemu::write(name);
	qemu::write(": ");
	qemu::write_unsigned(delay - 1);
	qemu::write(" calls interrupted\n");
	if (delay - 1 < min_interrupted)
		fail("too few calls interrupted to mean anything", checked_index);
}

} // namespace

extern "C" [[gnu::noinline]] void write_small(std::uint32_t seq)
{
	kymo::emit(small_event{seq, ~seq});
}

extern "C" [[gnu::noinline]] void write_large(std::uint32_t seq)
{
	large_event event{};
	event.seq = seq;
	for (std::uint32_t w = 0; w < event.words.size(); ++w)
		event.words[w] = pattern(seq, w);
	kymo::emit(event);
}

void test_interrupt()
{
	read_published();
	isr_event event{};
	event.seq = isr_written;
	for (std::uint32_t w = 0; w < event.words.size(); ++w)
		event.words[w] = pattern(~event.seq, w);
	kymo::emit(event);
	++isr_written;
}

void test_main()
{
	preempt::calibrate();

	// Start short of 2^32 so the index wraps during the run.
	std::uint32_t const start = 0U - (7U * KYMO_RING_SLOTS) - 3U;
	kymo::trace_ring.write_index.store(start, std::memory_order_relaxed);
	checked_index = start;

	std::uint32_t seq = 0;
	run("one-slot writer", kymo::type_id<small_event>(), write_small, seq);
	run("three-slot writer", kymo::type_id<large_event>(), write_large, seq);
	if (kymo::trace_ring.write_index.load(std::memory_order_relaxed) > start)
		fail("the index did not wrap", checked_index);
	qemu::finish(true);
}
