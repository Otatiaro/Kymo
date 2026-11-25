/**
 * @file    kymo.hpp
 * @brief   Kymo on the target: the application's tracer writes each event to
 *          the channel its family is routed to.
 *
 * An event is any trivially copyable struct; the host finds its layout and
 * name in the ELF, through its type id (type_id.hpp) and the DWARF.
 *
 * An event belongs to a family (control.hpp): its static member kymo_family,
 * or family::application. The control word enables families one by one, and
 * kymo::freeze() stops everything.
 *
 * A channel is anything with a `write(type, timestamp, event)`; a ring
 * (ring.hpp) is the channel the host drains through the debug port. The
 * application declares its channels and routes each family to one by
 * overloading `channel`; tracer gives it `emit`:
 *
 *     inline kymo::ring<256> os;
 *     inline uart_channel serial;
 *
 *     struct trace : kymo::tracer<trace>
 *     {
 *         static auto& channel(kymo::on<kymo::family::application>) { return serial; }
 *         template<kymo::family F>
 *         static auto& channel(kymo::on<F>) { return os; }
 *     };
 *
 *     trace::emit(event);
 *
 * A family with no overload does not compile once one of its events is
 * emitted; the generic overload routes every family not named.
 *
 * Timestamps come from the DWT cycle counter, which the program enables
 * (DEMCR.TRCENA, DWT_CTRL.CYCCNTENA).
 */
#pragma once

#include <kymo/control.hpp>   // IWYU pragma: export
#include <kymo/ring.hpp>      // IWYU pragma: export
#include <kymo/type_id.hpp>   // IWYU pragma: export

#include <atomic>
#include <cstdint>

namespace kymo
{

/// Address of DWT_CYCCNT, the cycle counter (ARMv7-M and ARMv8-M).
constexpr std::uintptr_t dwt_cyccnt = 0xE0001004;

/** @brief The DWT cycle counter. */
[[gnu::always_inline]] inline std::uint32_t timestamp()
{
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): a register at a fixed address
	return *reinterpret_cast<std::uint32_t const volatile*>(dwt_cyccnt);
}

/** @brief The tag a tracer's `channel` overloads take, one per family. */
template<family Which>
struct on
{
};

/**
 * @brief Gives @p Trace, the application's tracer, `emit`. @p Trace routes
 *        each family with a static `channel(on<F>)` that returns the channel.
 */
template<typename Trace>
struct tracer
{
	/**
	 * @brief Writes @p event to its family's channel, unless its family is
	 *        disabled or the trace frozen. Safe from any priority, any context.
	 */
	template<typename Event>
	[[gnu::always_inline]] static void emit(Event const& event)
	{
		if ((control.load(std::memory_order_relaxed) & bit(family_of<Event>)) == 0)
			return;
		Trace::channel(on<family_of<Event>>{}).write(type_id_word<Event>(), timestamp(), event);
	}

private:
	tracer() = default;
	friend Trace;
};

} // namespace kymo
