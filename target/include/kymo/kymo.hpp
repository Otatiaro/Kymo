/**
 * @file    kymo.hpp
 * @brief   Kymo on the target: kymo::emit(event) writes any event to the
 *          trace ring.
 *
 * An event is any trivially copyable struct; the host finds its layout and
 * name in the ELF, through its type id (type_id.hpp) and the DWARF.
 *
 * An event belongs to a family (control.hpp): its static member kymo_family,
 * or family::application. The control word enables families one by one, and
 * kymo::freeze() stops everything.
 *
 * Configuration, the same for every translation unit of the program:
 * - KYMO_RING_SLOTS: slots in the ring, a power of two (default 256, 4 KiB);
 * - KYMO_MAX_EVENT_SLOTS: slots the largest event takes (default 4, that is
 *   44 bytes of payload); the ring has that many slots, less one, past its
 *   end;
 * - KYMO_CONTROL_DEFAULT: the control word from reset (default: everything).
 *
 * Timestamps come from the DWT cycle counter, which the program or the host
 * enables (DEMCR.TRCENA, DWT_CTRL.CYCCNTENA).
 */
#pragma once

#include <kymo/control.hpp>
#include <kymo/ring.hpp>
#include <kymo/type_id.hpp>

#include <cstdint>

#ifndef KYMO_RING_SLOTS
#define KYMO_RING_SLOTS 256
#endif

#ifndef KYMO_MAX_EVENT_SLOTS
#define KYMO_MAX_EVENT_SLOTS 4
#endif

namespace kymo
{

/// The ring the host drains. Zeroed at reset: no slot looks written.
inline ring<KYMO_RING_SLOTS, KYMO_MAX_EVENT_SLOTS> trace_ring;

/** @brief The DWT cycle counter. */
[[gnu::always_inline]] inline std::uint32_t timestamp()
{
	return *reinterpret_cast<std::uint32_t const volatile*>(0xE0001004u);
}

/**
 * @brief Writes @p event to the trace, unless its family is disabled or the
 *        trace frozen. Safe from any priority, any context.
 */
template<typename Event>
[[gnu::always_inline]] inline void emit(Event const& event)
{
	if ((control.load(std::memory_order_relaxed) & bit(family_of<Event>)) == 0)
		return;
	trace_ring.write(type_id<Event>(), timestamp(), event);
}

} // namespace kymo
