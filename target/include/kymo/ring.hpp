/**
 * @file    ring.hpp
 * @brief   The trace ring: one overwrite ring of fixed 16-byte slots, filled
 *          lock-free from any context and drained by the host through the
 *          debug port while the core runs.
 *
 * An event takes one slot or several consecutive ones. Writing it takes
 * three steps:
 *  1. reserve its slot indexes with a single atomic add (LDREX/STREX on
 *     Cortex-M, retried on contention, interrupts never masked);
 *  2. fill the timestamp and the payload;
 *  3. publish the headers, each after a release barrier, the first slot's
 *     last. A header carries the lap of its slot index, so a slot whose
 *     header shows the expected lap is complete.
 *
 * Every slot starts with a header, so every slot boundary is a point where
 * the host can pick up the stream: the head bit marks an event's first slot.
 * The first slot holds the timestamp and 8 bytes of payload; each following
 * slot holds 12 more.
 *
 * An event is never cut by the end of the ring: the ring has a guard zone
 * past its last slot, one slot short of the largest event, and an event that
 * starts near the end runs on into it. The slots it would have wrapped onto
 * are left as they were, with an older lap; the host reads the guard zone
 * instead. The writer never tests for the end.
 *
 * The host knows a slot it read is intact when the write index read after the
 * slot has not gone past it by a full ring: no writer could have reserved the
 * slot again in the meantime. Every index the host gives up on is lost, so
 * losses are counted exactly without any counter on the target.
 */
#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <type_traits>

namespace kymo
{

/** @brief One slot. The header is written last; see the file comment. */
struct slot
{
	/// Bits 0-15: event type. Bit 16: head (first slot of an event).
	/// Bits 17-31: lap of the slot index plus one, modulo 2^15, so that a
	/// zeroed slot never passes for a written one.
	std::atomic<std::uint32_t> header;
	/// In an event's first slot: the timestamp, then 8 bytes of payload.
	/// In the slots that follow: 12 bytes of payload.
	std::uint32_t words[3];
};
static_assert(sizeof(slot) == 16, "the host reads 16-byte slots");

constexpr std::uint32_t head_bit = 1u << 16;
constexpr unsigned lap_shift = 17;

/// Payload bytes in an event's first slot, and in each slot after it.
constexpr std::size_t head_payload = 8;
constexpr std::size_t tail_payload = 12;

/** @brief The slots an event of @p Event takes. */
template<typename Event>
constexpr std::size_t slots_for = sizeof(Event) <= head_payload
	? 1
	: 1 + (sizeof(Event) - head_payload + tail_payload - 1) / tail_payload;

/**
 * @brief The ring. @p Slots is a power of two so the index wraps cleanly at
 *        2^32 and a slot is found with a mask. @p MaxEventSlots bounds the
 *        events it takes and sizes the guard zone.
 *
 * The layout is read by the host from the ELF: keep the fields in this order.
 */
template<std::size_t Slots, std::size_t MaxEventSlots = 1>
struct ring
{
	static_assert(Slots >= 2 && (Slots & (Slots - 1)) == 0, "Slots must be a power of two");
	static_assert(MaxEventSlots >= 1 && MaxEventSlots <= Slots, "MaxEventSlots must be from 1 to Slots");
	static constexpr unsigned log2_slots = __builtin_ctz(Slots);
	static constexpr std::size_t guard_slots = MaxEventSlots - 1;

	std::atomic<std::uint32_t> write_index;   ///< next index to reserve
	slot slots[Slots + guard_slots];

	/** @brief The header of the slot at @p index. */
	static constexpr std::uint32_t header(std::uint16_t type, std::uint32_t index, bool head)
	{
		std::uint32_t const lap = ((index >> log2_slots) + 1) & 0x7fffu;
		return type | (head ? head_bit : 0) | (lap << lap_shift);
	}

	/** @brief Writes @p event. Safe from any priority, any context. */
	template<typename Event>
	void write(std::uint16_t type, std::uint32_t timestamp, Event const& event)
	{
		static_assert(std::is_trivially_copyable<Event>::value, "an event is copied byte for byte");
		constexpr std::size_t count = slots_for<Event>;
		static_assert(count <= MaxEventSlots, "the event is larger than the ring's MaxEventSlots");

		std::uint32_t const index = write_index.fetch_add(count, std::memory_order_relaxed);
		slot* const first = &slots[index & (Slots - 1)];
		auto const* const bytes = reinterpret_cast<unsigned char const*>(&event);

		first->words[0] = timestamp;
		std::memcpy(&first->words[1], bytes, sizeof(Event) < head_payload ? sizeof(Event) : head_payload);
		for (std::size_t i = 1; i < count; ++i)
		{
			std::size_t const offset = head_payload + (i - 1) * tail_payload;
			std::size_t const left = sizeof(Event) - offset;
			std::memcpy(first[i].words, bytes + offset, left < tail_payload ? left : tail_payload);
			first[i].header.store(header(type, index + i, false), std::memory_order_release);
		}
		first->header.store(header(type, index, true), std::memory_order_release);
	}
};

} // namespace kymo
