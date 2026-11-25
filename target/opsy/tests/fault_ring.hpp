/**
 * @file    fault_ring.hpp
 * @brief   A kymo::fault read back from the traced images' ring, for the
 *          images that check what the adapter recorded.
 */
#pragma once

#include <opsy_trace.hpp>

#include <kymo/fault.hpp>
#include <kymo/ring.hpp>
#include <kymo/type_id.hpp>

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>

/**
 * @brief The kymo::fault written at @p index, in @p out; false when the slots
 *        there do not hold one, head first, each with its index's lap.
 */
inline bool read_fault(std::uint32_t index, kymo::fault& out)
{
	using ring_t = decltype(trace_ring);
	constexpr std::size_t count = kymo::slots_for<kymo::fault>;
	std::uint32_t const type = kymo::type_id<kymo::fault>();
	std::array<unsigned char, count * kymo::tail_payload> bytes{};
	for (std::size_t i = 0; i < count; ++i)
	{
		std::uint32_t const at = index + static_cast<std::uint32_t>(i);
		auto const& slot = trace_ring.slots[at & (ring_slots - 1)];
		if (slot.header.load(std::memory_order_acquire) != ring_t::header(type, at, i == 0))
			return false;
		if (i == 0)
			std::memcpy(bytes.data(), &slot.words[1], kymo::head_payload);
		else
			std::memcpy(&bytes[kymo::head_payload + ((i - 1) * kymo::tail_payload)], slot.words.data(), kymo::tail_payload);
	}
	std::memcpy(&out, bytes.data(), sizeof(out));
	return true;
}
