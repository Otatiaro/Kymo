/**
 * @file    opsy_trace.hpp
 * @brief   The tracer of OpSy's traced images: every family to one ring of
 *          256 slots.
 */
#pragma once

#include <kymo/kymo.hpp>   // IWYU pragma: export

#include <cstdint>

/// Slots of the images' ring.
constexpr std::uint32_t ring_slots = 256;

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
