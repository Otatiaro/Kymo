/**
 * @file    kymo_zephyr_trace.hpp
 * @brief   The test application's tracer, given to the adapter's unit: every
 *          family to one ring of 1024 slots.
 */
#pragma once

#include <kymo/kymo.hpp>   // IWYU pragma: export

#include <cstdint>

/// Slots of the ring: more than the application emits events.
constexpr std::uint32_t ring_slots = 1024;

/// The ring the host drains.
inline kymo::ring<ring_slots> trace_ring;   // NOLINT(cppcoreguidelines-avoid-non-const-global-variables): written by every emit

/** @brief Routes every family to trace_ring. */
struct application_trace : kymo::tracer<application_trace>
{
	template<kymo::family F>
	static auto& channel(kymo::on<F> /*family*/)
	{
		return trace_ring;
	}
};

namespace kymo::zephyr
{
using trace = ::application_trace;
}
