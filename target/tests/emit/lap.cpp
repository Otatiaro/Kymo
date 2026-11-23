/**
 * @file    lap.cpp
 * @brief   A writer preempted for a full lap of the ring: nothing the host
 *          takes as intact may hold another event's bytes.
 *
 * A writer emits a three-slot event from thread mode. A first interrupt lands
 * on one of its instructions and emits a full ring of one-slot events, so the
 * slots the writer reserved are reserved again, by newer events, while the
 * writer is still in flight. It arms a second interrupt, which lands on a
 * later instruction of the writer and reads the ring as the host does: an
 * event is intact when its headers carry the lap of its index and the write
 * index, read after the event, has not gone past it by a full ring. Every
 * event it takes as intact must hold exactly its own bytes.
 *
 * Both interrupts sweep every instruction of the writer (preempt.hpp).
 */
#include "../qemu/preempt.hpp"
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

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
	std::uint32_t words[7];
};

static_assert(kymo::slots_for<lap_event> == 1, "");
static_assert(kymo::slots_for<large_event> == 3, "");

constexpr std::uint32_t pattern(std::uint32_t seq, std::uint32_t word)
{
	return (seq * 0x9E3779B1u) ^ (word * 0x85EBCA6Bu) ^ 0xA5A5A5A5u;
}

constexpr std::uint32_t slots = KYMO_RING_SLOTS;
constexpr std::uint32_t slot_mask = slots - 1;
constexpr std::uint32_t max_delay = 2000;

using ring_type = decltype(kymo::trace_ring);

std::uint32_t volatile second_delay;   // delay of the second interrupt
std::uint32_t volatile stage;          // interrupts taken in the current call
std::uint32_t lap_seq;
std::uint32_t reads;                   // events the reader took as intact

std::uint32_t volatile& reg(std::uint32_t address)
{
	return *reinterpret_cast<std::uint32_t volatile*>(address);
}

[[noreturn]] void fail(char const* what, std::uint32_t first, std::uint32_t second)
{
	qemu::write("FAIL with delays ");
	qemu::write_unsigned(first);
	qemu::write(" and ");
	qemu::write_unsigned(second);
	qemu::write(": ");
	qemu::write(what);
	qemu::write("\n");
	qemu::finish(false);
}

std::uint32_t current_first_delay;

/** @brief Reads the ring as the host does, and checks every intact event. */
void read_as_host()
{
	std::uint32_t const end = kymo::trace_ring.write_index.load(std::memory_order_acquire);
	for (std::uint32_t index = end - slots; index != end; ++index)
	{
		kymo::slot const* const first = &kymo::trace_ring.slots[index & slot_mask];
		std::uint32_t const head = first->header.load(std::memory_order_acquire);
		std::uint16_t const type = static_cast<std::uint16_t>(head & 0xffffu);
		if (head != ring_type::header(type, index, true))
			continue;

		alignas(4) unsigned char bytes[sizeof(large_event)];
		std::size_t size;
		if (type == kymo::type_id<lap_event>())
			size = sizeof(lap_event);
		else if (type == kymo::type_id<large_event>())
			size = sizeof(large_event);
		else
			continue;
		std::uint32_t const count = size == sizeof(large_event) ? kymo::slots_for<large_event> : 1;

		bool whole = true;
		std::memcpy(bytes, &first->words[1], size < kymo::head_payload ? size : kymo::head_payload);
		for (std::uint32_t i = 1; i < count; ++i)
		{
			if (first[i].header.load(std::memory_order_acquire) != ring_type::header(type, index, false))
				whole = false;
			std::size_t const offset = kymo::head_payload + (i - 1) * kymo::tail_payload;
			std::size_t const left = size - offset;
			std::memcpy(bytes + offset, first[i].words, left < kymo::tail_payload ? left : kymo::tail_payload);
		}
		if (!whole)
			continue;
		std::uint32_t const after = kymo::trace_ring.write_index.load(std::memory_order_acquire);
		if (after - index > slots)
			continue;

		++reads;
		if (size == sizeof(lap_event))
		{
			lap_event e;
			std::memcpy(&e, bytes, sizeof e);
			if (e.check != ~e.seq)
				fail("an intact one-slot event holds other bytes", current_first_delay, second_delay);
		}
		else
		{
			large_event e;
			std::memcpy(&e, bytes, sizeof e);
			for (std::uint32_t w = 0; w < 7; ++w)
				if (e.words[w] != pattern(e.seq, w))
					fail("an intact three-slot event holds other bytes", current_first_delay, second_delay);
		}
	}
}

} // namespace

extern "C" [[gnu::noinline]] void write_large(std::uint32_t seq)
{
	large_event e;
	e.seq = seq;
	for (std::uint32_t w = 0; w < 7; ++w)
		e.words[w] = pattern(seq, w);
	kymo::emit(e);
}

void test_interrupt()
{
	if (stage++ == 0)
	{
		for (std::uint32_t i = 0; i < slots; ++i)
		{
			kymo::emit(lap_event{lap_seq, ~lap_seq});
			++lap_seq;
		}
		// The second interrupt, from the same SysTick, after second_delay ticks.
		// Enabled from its own handler, SysTick also pends at once under QEMU:
		// that pending interrupt is cleared, so only the delayed one remains.
		reg(0xE000E014u) = second_delay;
		reg(0xE000E018u) = 0;
		reg(0xE000E010u) = 7u;
		reg(0xE000ED04u) = 1u << 25;
	}
	else
		read_as_host();
}

void test_main()
{
	preempt::calibrate();

	std::uint32_t seq = 0;
	std::uint32_t pairs = 0;
	for (std::uint32_t first = 1;; ++first)
	{
		if (first > max_delay)
			fail("the writer outlasted every delay", first, 0);
		current_first_delay = first;
		for (std::uint32_t second = 1;; ++second)
		{
			if (second > max_delay)
				fail("the writer outlasted every second delay", first, second);
			stage = 0;
			second_delay = second;
			preempt::call(first, write_large, seq++);
			if (stage < 2)
				break;   // the call ended before the second interrupt
			++pairs;
		}
		if (stage == 0)
			break;   // the call ended before the first interrupt
	}
	qemu::write("pairs of interrupts: ");
	qemu::write_unsigned(pairs);
	qemu::write(", events read as intact: ");
	qemu::write_unsigned(reads);
	qemu::write("\n");
	qemu::finish(true);
}
