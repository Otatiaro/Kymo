/**
 * @file    ring.hpp
 * @brief   STUDY PROTOTYPE: a ring of small cells whose header carries the
 *          low bits of the timestamp, with reference records that carry it
 *          whole. Not for merging; measured by target/tests/cost.
 *
 * A cell is a header word and KYMO_CELL_WORDS payload words. The header holds
 * the type, the head bit, the lap of the cell's index plus one and, in its top
 * KYMO_STAMP_BITS bits, the low bits of the timestamp. A reference record is
 * a one-cell event of the reserved type whose payload is the whole timestamp;
 * a writer writes one first when its timestamp is more than `reference_after`
 * past the last reference written, so every event lies within that of the
 * reference before it in index order (less a preemption of up to a quarter of
 * the window between reading the timestamp and reserving).
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

#ifndef KYMO_CELL_WORDS
#define KYMO_CELL_WORDS 1
#endif
#ifndef KYMO_STAMP_BITS
#define KYMO_STAMP_BITS 16
#endif
#ifndef KYMO_LAP_BITS
#define KYMO_LAP_BITS 3
#endif

namespace kymo
{

constexpr std::size_t slot_words = KYMO_CELL_WORDS;
constexpr std::size_t slot_size = 4 * (slot_words + 1);

struct slot
{
	std::atomic<std::uint32_t> header{0};
	std::array<std::uint32_t, slot_words> words{};
};
static_assert(sizeof(slot) == slot_size);

constexpr unsigned stamp_bits = KYMO_STAMP_BITS;
constexpr unsigned lap_bits = KYMO_LAP_BITS;
constexpr unsigned stamp_shift = 32 - stamp_bits - lap_bits;
constexpr unsigned type_bits = 32 - stamp_bits - lap_bits - 1;
constexpr std::uint32_t head_bit = 1U << type_bits;
constexpr unsigned lap_shift = 32 - lap_bits;
constexpr std::uint32_t lap_mask = (1U << lap_bits) - 1;
constexpr std::uint32_t stamp_mask = ((1U << stamp_bits) - 1) << stamp_shift;
constexpr std::uint32_t reference_type = (1U << type_bits) - 1;
/// A reference is written once the timestamp is this far past the last one.
constexpr std::uint32_t reference_after = (3U << stamp_bits) / 4;

constexpr std::size_t head_payload = sizeof(slot::words);
constexpr std::size_t tail_payload = sizeof(slot::words);

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

template<std::size_t Size, std::size_t... Words>
[[gnu::always_inline]] inline void copy_words(std::uint32_t* words, unsigned char const* bytes,
                                              std::index_sequence<Words...> /*words*/)
{
	constexpr std::size_t word = sizeof(std::uint32_t);
	(std::memcpy(words + Words, bytes + (Words * word), std::min(Size - (Words * word), word)), ...);
}

template<std::size_t Size>
[[gnu::always_inline]] inline void copy_words(std::uint32_t* words, unsigned char const* bytes)
{
	copy_words<Size>(words, bytes, std::make_index_sequence<(Size + sizeof(std::uint32_t) - 1) / sizeof(std::uint32_t)>{});
}

template<typename Event>
constexpr std::size_t slots_for = sizeof(Event) <= head_payload
	? 1
	: 1 + ((sizeof(Event) - head_payload + tail_payload - 1) / tail_payload);

template<std::size_t Slots>
struct ring
{
	static_assert(Slots >= 2 && (Slots & (Slots - 1)) == 0, "Slots must be a power of two");
	static constexpr unsigned log2_slots = log2_of(Slots);

	std::atomic<std::uint32_t> write_index{0};
	std::atomic<std::uint32_t> lapped{0};
	/// The timestamp of the last reference written; the target's only.
	std::atomic<std::uint32_t> reference{0};
	std::array<slot, Slots> slots;

	[[gnu::always_inline]] static constexpr std::uint32_t header(std::uint32_t type, std::uint32_t index, bool head)
	{
		std::uint32_t const lap = (index >> log2_slots) + 1;
		return type | (head ? head_bit : 0) | (lap << lap_shift);
	}

	template<typename Event>
	[[gnu::always_inline]] void write(std::uint32_t type, std::uint32_t timestamp, Event const& event)
	{
		static_assert(std::is_trivially_copyable_v<Event>);
		constexpr std::size_t count = slots_for<Event>;
		static_assert(count <= Slots);

		if (__builtin_expect(timestamp - reference.load(std::memory_order_relaxed) > reference_after, 0))
			write_reference(timestamp);

		std::uint32_t const index = write_index.fetch_add(count, std::memory_order_relaxed);
		slot& first = slots[index & (Slots - 1)];
		auto const* const bytes = reinterpret_cast<unsigned char const*>(&event);

		copy_words<std::min(sizeof(Event), head_payload)>(first.words.data(), bytes);
		write_tails<Event>(type, index, bytes, std::make_index_sequence<count - 1>{});
		first.header.store(header(type, index, true) | ((timestamp << stamp_shift) & stamp_mask), std::memory_order_release);
		std::atomic_signal_fence(std::memory_order_seq_cst);
		if (write_index.load(std::memory_order_relaxed) - index > Slots)
			lapped.fetch_add(1, std::memory_order_relaxed);
	}

	/** @brief Writes a reference record: the whole timestamp. */
	[[gnu::noinline]] void write_reference(std::uint32_t timestamp)
	{
		std::uint32_t const index = write_index.fetch_add(1, std::memory_order_relaxed);
		slot& cell = slots[index & (Slots - 1)];
		cell.words[0] = timestamp;
		cell.header.store(header(reference_type, index, true) | ((timestamp << stamp_shift) & stamp_mask), std::memory_order_release);
		std::atomic_signal_fence(std::memory_order_seq_cst);
		if (write_index.load(std::memory_order_relaxed) - index > Slots)
			lapped.fetch_add(1, std::memory_order_relaxed);
		reference.store(timestamp, std::memory_order_relaxed);
	}

private:
	template<typename Event, std::size_t... Tails>
	[[gnu::always_inline]] void write_tails([[maybe_unused]] std::uint32_t type, [[maybe_unused]] std::uint32_t index,
	                                        [[maybe_unused]] unsigned char const* bytes,
	                                        std::index_sequence<Tails...> /*tails*/)
	{
		(write_tail<Event, Tails + 1>(type, index, bytes), ...);
	}

	template<typename Event, std::size_t I>
	[[gnu::always_inline]] void write_tail(std::uint32_t type, std::uint32_t index, unsigned char const* bytes)
	{
		constexpr std::size_t offset = head_payload + ((I - 1) * tail_payload);
		constexpr std::size_t left = sizeof(Event) - offset;
		std::uint32_t const tail_index = index + I;
		slot& next = slots[tail_index & (Slots - 1)];
		std::memcpy(next.words.data(), bytes + offset, std::min(left, tail_payload));
		next.header.store(header(type, tail_index, false), std::memory_order_release);
	}
};

} // namespace kymo
