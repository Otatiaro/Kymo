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
 *     last. Every header of an event carries the lap of its first slot's
 *     index, so a slot whose header shows the expected lap is complete.
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
 * instead, where the slots carry the lap of the event's first slot, not the
 * lap their own index would have. The writer never tests for the end.
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
#include <utility>

namespace kymo
{

/** @brief One slot. The header is written last; see the file comment. */
struct slot
{
	/// Bits 0-15: event type. Bit 16: head (first slot of an event).
	/// Bits 17-31: lap of the index of the event's first slot plus one,
	/// modulo 2^15, so that a zeroed slot never passes for a written one.
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

	/** @brief The header of a slot of the event whose first slot is at @p index. */
	static constexpr std::uint32_t header(std::uint32_t type, std::uint32_t index, bool head)
	{
		std::uint32_t const lap = ((index >> log2_slots) + 1) & 0x7fffu;
		return type | (head ? head_bit : 0) | (lap << lap_shift);
	}

	/** @brief Writes @p event. Safe from any priority, any context. */
	template<typename Event>
	[[gnu::always_inline]] void write(std::uint32_t type, std::uint32_t timestamp, Event const& event)
	{
		static_assert(std::is_trivially_copyable<Event>::value, "an event is copied byte for byte");
		constexpr std::size_t count = slots_for<Event>;
		static_assert(count <= MaxEventSlots, "the event is larger than the ring's MaxEventSlots");

		std::uint32_t const index = write_index.fetch_add(count, std::memory_order_relaxed);
		slot* const first = &slots[index & (Slots - 1)];
		auto const* const bytes = reinterpret_cast<unsigned char const*>(&event);

		first->words[0] = timestamp;
		std::memcpy(&first->words[1], bytes, sizeof(Event) < head_payload ? sizeof(Event) : head_payload);
		write_tails<Event>(first, header(type, index, false), bytes, std::make_index_sequence<count - 1>{});
		first->header.store(header(type, index, true), std::memory_order_release);
	}

private:
	/** @brief Fills and publishes the slots after the first, one per @p Tails
	 *         (none for a one-slot event), each with the header @p tail. */
	template<typename Event, std::size_t... Tails>
	[[gnu::always_inline]] static void write_tails([[maybe_unused]] slot* first, [[maybe_unused]] std::uint32_t tail,
	                                               [[maybe_unused]] unsigned char const* bytes,
	                                               std::index_sequence<Tails...>)
	{
		(write_tail<Event, Tails + 1>(first, tail, bytes), ...);
	}

	/** @brief Fills and publishes slot @p I of an event, I > 0. */
	template<typename Event, std::size_t I>
	[[gnu::always_inline]] static void write_tail(slot* first, std::uint32_t tail, unsigned char const* bytes)
	{
		constexpr std::size_t offset = head_payload + (I - 1) * tail_payload;
		constexpr std::size_t left = sizeof(Event) - offset;
		std::memcpy(first[I].words, bytes + offset, left < tail_payload ? left : tail_payload);
		first[I].header.store(tail, std::memory_order_release);
	}
};

} // namespace kymo
