/**
 * @file    opsy_trace.hpp
 * @brief   The tracer of OpSy's traced images: every family to one ring, of
 *          256 slots unless the image defines KYMO_TEST_RING_SLOTS.
 */
#pragma once

#include <kymo/kymo.hpp>   // IWYU pragma: export

#include <cstdint>

/// Slots of the images' ring.
#ifdef KYMO_TEST_RING_SLOTS
constexpr std::uint32_t ring_slots = KYMO_TEST_RING_SLOTS;
#else
constexpr std::uint32_t ring_slots = 256;
#endif

/// The ring the host drains.
inline kymo::ring<ring_slots> trace_ring;

/** @brief Routes every family to trace_ring. */
struct trace : kymo::tracer<trace>
{
	template<kymo::family F>
	static auto& channel(kymo::on<F> /*family*/)
	{
		return trace_ring;
	}
};
