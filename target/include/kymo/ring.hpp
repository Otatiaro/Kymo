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
 *     last. Every header carries the lap of its own slot's index, so a slot
 *     whose header shows the expected lap is complete.
 *
 * Every slot starts with a header, so every slot boundary is a point where
 * the host can pick up the stream: the head bit marks an event's first slot.
 * The first slot holds the timestamp and 8 bytes of payload; each following
 * slot holds 12 more.
 *
 * An event that reaches the end of the ring goes on at slot 0: slot I of the
 * event whose first index is `index` is at `(index + I) mod Slots`, and carries
 * the lap of `index + I`: a slot past the end is told from one the previous
 * lap left there by its header alone. The writer never tests for the end; an
 * event only has to fit in the ring, which is checked at compile time.
 *
 * The host knows a slot it read is intact when the write index read after the
 * slot has not gone past it by a full ring: no writer could have reserved the
 * slot again in the meantime. Every index the host gives up on is lost, so
 * losses are counted exactly without any counter on the target.
 *
 * A writer preempted for a full lap writes on into slots newer events have
 * reserved again. Once its first slot is published, it reads the write index
 * again: past its index by more than a ring, it was lapped, and it counts
 * itself in `lapped`. The reservation that lapped it came before its last
 * write, so before that read: every writer that wrote into slots no longer
 * its own is counted. One lapped between publishing and the read is counted
 * too, though it wrote nothing stale.
 */
#pragma once

#include <algorithm>
#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <type_traits>
#include <utility>

namespace kymo
{

/// Bytes of a slot, as the host reads them.
constexpr std::size_t slot_size = 16;
/// Words of a slot after its header.
constexpr std::size_t slot_words = 3;

/** @brief One slot. The header is written last; see the file comment. */
struct slot
{
	/// Bits 0-15: event type. Bit 16: head (first slot of an event).
	/// Bits 17-31: lap of the slot's index plus one, modulo 2^15, so that a
	/// zeroed slot never passes for a written one.
	std::atomic<std::uint32_t> header{0};
	/// In an event's first slot: the timestamp, then 8 bytes of payload.
	/// In the slots that follow: 12 bytes of payload.
	std::array<std::uint32_t, slot_words> words{};
};
static_assert(sizeof(slot) == slot_size, "the host reads 16-byte slots");

constexpr unsigned type_bits = 16;
constexpr std::uint32_t head_bit = 1U << type_bits;
constexpr unsigned lap_shift = type_bits + 1;
constexpr std::uint32_t lap_mask = (1U << (32 - lap_shift)) - 1;

/// Payload bytes in an event's first slot, and in each slot after it.
constexpr std::size_t head_payload = sizeof(slot::words) - sizeof(std::uint32_t);
constexpr std::size_t tail_payload = sizeof(slot::words);

/** @brief The base-2 logarithm of @p n, a power of two. */
constexpr unsigned log2_of(std::size_t n)
{
	unsigned bits = 0;
	while (n > 1)
	{
		n >>= 1U;
		++bits;
	}
	return bits;
}

/** @brief The slots an event of @p Event takes. */
template<typename Event>
constexpr std::size_t slots_for = sizeof(Event) <= head_payload
	? 1
	: 1 + ((sizeof(Event) - head_payload + tail_payload - 1) / tail_payload);

/**
 * @brief A ring, one of the channels events are written to. @p Slots is a
 *        power of two so the index wraps cleanly at 2^32 and a slot is found
 *        with a mask.
 *
 * The host finds every object of this type in the ELF, reads @p Slots from the
 * template parameter and the layout from the DWARF: keep the fields in this
 * order. A ring at namespace scope is zeroed at reset: no slot looks written.
 */
template<std::size_t Slots>
struct ring
{
	static_assert(Slots >= 2 && (Slots & (Slots - 1)) == 0, "Slots must be a power of two");
	static constexpr unsigned log2_slots = log2_of(Slots);

	std::atomic<std::uint32_t> write_index{0};   ///< next index to reserve
	std::atomic<std::uint32_t> lapped{0};        ///< writers lapped by the ring, modulo 2^32
	std::array<slot, Slots> slots;

	/** @brief The header of the slot at @p index. */
	static constexpr std::uint32_t header(std::uint32_t type, std::uint32_t index, bool head)
	{
		std::uint32_t const lap = ((index >> log2_slots) + 1) & lap_mask;
		return type | (head ? head_bit : 0) | (lap << lap_shift);
	}

	/** @brief Writes @p event. Safe from any priority, any context. */
	template<typename Event>
	[[gnu::always_inline]] void write(std::uint32_t type, std::uint32_t timestamp, Event const& event)
	{
		static_assert(std::is_trivially_copyable_v<Event>, "an event is copied byte for byte");
		constexpr std::size_t count = slots_for<Event>;
		static_assert(count <= Slots, "the event does not fit in the ring");

		std::uint32_t const index = write_index.fetch_add(count, std::memory_order_relaxed);
		slot& first = slots[index & (Slots - 1)];
		// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast): an event is copied byte for byte, which aliasing allows
		auto const* const bytes = reinterpret_cast<unsigned char const*>(&event);

		first.words[0] = timestamp;
		std::memcpy(&first.words[1], bytes, std::min(sizeof(Event), head_payload));
		write_tails<Event>(type, index, bytes, std::make_index_sequence<count - 1>{});
		first.header.store(header(type, index, true), std::memory_order_release);
		// Keeps the read below after the store above: a writer lapped before
		// its last write is never missed.
		std::atomic_signal_fence(std::memory_order_seq_cst);
		if (write_index.load(std::memory_order_relaxed) - index > Slots)
			lapped.fetch_add(1, std::memory_order_relaxed);
	}

private:
	/** @brief Fills and publishes the slots after the first, one per @p Tails
	 *         (none for a one-slot event). */
	template<typename Event, std::size_t... Tails>
	[[gnu::always_inline]] void write_tails([[maybe_unused]] std::uint32_t type, [[maybe_unused]] std::uint32_t index,
	                                        [[maybe_unused]] unsigned char const* bytes,
	                                        std::index_sequence<Tails...> /*tails*/)
	{
		(write_tail<Event, Tails + 1>(type, index, bytes), ...);
	}

	/** @brief Fills and publishes slot @p I of the event whose first index is
	 *         @p index, I > 0. */
	template<typename Event, std::size_t I>
	[[gnu::always_inline]] void write_tail(std::uint32_t type, std::uint32_t index, unsigned char const* bytes)
	{
		constexpr std::size_t offset = head_payload + ((I - 1) * tail_payload);
		constexpr std::size_t left = sizeof(Event) - offset;
		std::uint32_t const tail_index = index + I;
		slot& next = slots[tail_index & (Slots - 1)];
		// NOLINTNEXTLINE(cppcoreguidelines-pro-bounds-pointer-arithmetic): the event's bytes, past those already copied
		std::memcpy(next.words.data(), bytes + offset, std::min(left, tail_payload));
		next.header.store(header(type, tail_index, false), std::memory_order_release);
	}
};

} // namespace kymo
