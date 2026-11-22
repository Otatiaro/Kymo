/**
 * @file    control.hpp
 * @brief   The control word: which event families are recorded, in one
 *          32-bit word that every emit reads.
 *
 * Bit n enables the family whose value is n. A writer tests its event's bit
 * and records nothing when it is clear. Freezing clears every bit: nothing is
 * recorded until a family is enabled again.
 *
 * The host writes the word through the debug port, at the address the ELF
 * gives for kymo::control (an aligned 32-bit write is atomic with respect to
 * the core), then reads it back. To restore a filter after a freeze, the host
 * keeps the value it wrote: the target keeps no copy.
 *
 * The application changes the word with enable(), disable() and freeze(),
 * from any context. A write by the host while the application changes the
 * word may be lost, as the core's exclusive monitor does not see the debug
 * port: the host's read back shows it.
 *
 * Keeping a recording after a fault is the application's job: enable() after
 * a freeze records again and overwrites the oldest events. An application
 * that freezes on a fault stops there, or resets once the host has read the
 * ring.
 *
 * The word holds KYMO_CONTROL_DEFAULT from reset, constant-initialised with
 * no startup code: a board with no host attached records with it.
 *
 * A writer that loads the word just before a freeze still writes its event
 * after it. That event is timestamped like any other.
 */
#pragma once

#include <atomic>
#include <cstdint>
#include <type_traits>

#ifndef KYMO_CONTROL_DEFAULT
#define KYMO_CONTROL_DEFAULT 0xffffffffu
#endif

namespace kymo
{

/** @brief Event families; the value is the family's bit in the control word. */
enum class family : std::uint8_t
{
	scheduling,   ///< a task takes or leaves the core, idle, creation, end, priority
	causality,    ///< a task becomes ready and why, starts waiting, is notified
	interrupts,   ///< entry and exit of each handler, SVC and its service
	exclusion,    ///< mutexes, priority inheritance, masked sections
	faults,       ///< traps, asserts, hardware faults
	application,  ///< marks, intervals, values, text
	meta,         ///< losses, active configuration, freeze, build id
};

/**
 * @brief The family of @p Event: its static member kymo_family, or
 *        family::application when it has none.
 *
 * A static member takes no room in the event; the host reads it from DWARF.
 */
template<typename Event, typename = void>
constexpr family family_of = family::application;

template<typename Event>
constexpr family family_of<Event, std::void_t<decltype(Event::kymo_family)>> = Event::kymo_family;

/** @brief The bit of @p f in the control word. */
constexpr std::uint32_t bit(family f)
{
	return 1u << static_cast<unsigned>(f);
}

/// The control word. The host finds it in the ELF.
inline std::atomic<std::uint32_t> control{KYMO_CONTROL_DEFAULT};

/** @brief Records the events of @p f. Safe from any priority, any context. */
inline void enable(family f)
{
	control.fetch_or(bit(f), std::memory_order_relaxed);
}

/** @brief Stops recording the events of @p f. Safe from any priority, any context. */
inline void disable(family f)
{
	control.fetch_and(~bit(f), std::memory_order_relaxed);
}

/** @brief Stops all recording. Safe from any priority, any context, a fault handler included. */
[[gnu::always_inline]] inline void freeze()
{
	control.store(0, std::memory_order_relaxed);
}

} // namespace kymo
