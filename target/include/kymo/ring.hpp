/**
 * @file    ring.hpp
 * @brief   The trace ring: one overwrite ring of fixed 16-byte slots, filled
 *          lock-free from any context and drained by the host through the
 *          debug port while the core runs.
 *
 * Writing an event takes three steps:
 *  1. reserve a slot index with a single atomic increment (LDREX/STREX on
 *     Cortex-M, retried on contention, interrupts never masked);
 *  2. fill the timestamp and the payload;
 *  3. publish the header, after a release barrier. The header carries the
 *     lap of the index, so a slot whose header shows the expected lap is
 *     complete.
 *
 * The host knows a slot it read is intact when the write index read after the
 * slot has not gone past it by a full ring: no writer could have reserved the
 * slot again in the meantime. Every index the host gives up on is a lost
 * event, so losses are counted exactly without any counter on the target.
 */
#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>

namespace kymo
{

/** @brief One slot. The header is written last; see the file comment. */
struct slot
{
	/// Bits 0-15: event type. Bit 16: head (first slot of an event).
	/// Bits 17-31: lap of the slot index plus one, modulo 2^15, so that a
	/// zeroed slot never passes for a written one.
	std::atomic<std::uint32_t> header;
	std::uint32_t timestamp;     ///< DWT cycle counter when the event was written
	std::uint32_t payload[2];
};
static_assert(sizeof(slot) == 16, "the host reads 16-byte slots");

constexpr std::uint32_t head_bit = 1u << 16;
constexpr unsigned lap_shift = 17;

/**
 * @brief The ring. @p Slots is a power of two so the index wraps cleanly at
 *        2^32 and a slot is found with a mask.
 *
 * The layout is read by the host from the ELF: keep the fields in this order.
 * The host derives the slot count from the size of the symbol.
 */
template<std::size_t Slots>
struct ring
{
	static_assert(Slots >= 2 && (Slots & (Slots - 1)) == 0, "Slots must be a power of two");
	static constexpr unsigned log2_slots = __builtin_ctz(Slots);

	std::atomic<std::uint32_t> write_index;   ///< next index to reserve
	slot slots[Slots];

	/** @brief Writes a one-slot event. Safe from any priority, any context. */
	void write(std::uint16_t type, std::uint32_t timestamp, std::uint32_t p0, std::uint32_t p1)
	{
		std::uint32_t const index = write_index.fetch_add(1, std::memory_order_relaxed);
		slot& s = slots[index & (Slots - 1)];
		s.timestamp = timestamp;
		s.payload[0] = p0;
		s.payload[1] = p1;
		std::uint32_t const lap = ((index >> log2_slots) + 1) & 0x7fffu;
		s.header.store(type | head_bit | (lap << lap_shift), std::memory_order_release);
	}
};

} // namespace kymo
