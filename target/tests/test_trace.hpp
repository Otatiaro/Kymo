/**
 * @file    test_trace.hpp
 * @brief   The tests' tracer: every family to one ring of 16 slots, small so
 *          that events keep running past its end.
 */
#pragma once

#include <kymo/kymo.hpp>   // IWYU pragma: export

#include <cstdint>

/// Slots of the tests' ring.
constexpr std::uint32_t ring_slots = 16;

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
