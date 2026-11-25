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
 * A duration is a pair of events: trace::begin(event) opens it, trace::end<Event>()
 * closes it, and `auto open = trace::scope(event);` closes it when `open`
 * goes out of scope. An end closes the last duration of its type still open,
 * wherever it was opened: durations of a type nest. With a key, an end closes
 * the one of its type opened with the same key, in any order:
 *
 *     trace::begin(batch{id, count}, buffer);
 *     trace::end<batch>(buffer);
 *
 * The wire carries begin_of<Event>, end_of<Event>, keyed_begin_of<Event> and
 * keyed_end_of<Event>; the host pairs them by their names in the DWARF.
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

/** @brief Opens a duration of @p Event, its fields those of @p event. */
template<typename Event>
struct begin_of
{
	static constexpr family kymo_family = family_of<Event>;
	Event event;
};

/** @brief Closes the last duration of @p Event still open. */
template<typename Event>
struct end_of
{
	static constexpr family kymo_family = family_of<Event>;
};

/** @brief Opens a duration of @p Event under a key of the application's. */
template<typename Event>
struct keyed_begin_of
{
	static constexpr family kymo_family = family_of<Event>;
	std::uint32_t kymo_key;
	Event event;
};

/** @brief Closes the duration of @p Event opened under the same key. */
template<typename Event>
struct keyed_end_of
{
	static constexpr family kymo_family = family_of<Event>;
	std::uint32_t kymo_key;
};

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

	/** @brief Opens a duration of @p Event, as emit() writes an event. */
	template<typename Event>
	[[gnu::always_inline]] static void begin(Event const& event)
	{
		emit(begin_of<Event>{event});
	}

	/** @brief Opens a duration of @p Event under @p key. */
	template<typename Event>
	[[gnu::always_inline]] static void begin(Event const& event, std::uint32_t key)
	{
		emit(keyed_begin_of<Event>{key, event});
	}

	/** @brief Closes the last duration of @p Event still open. */
	template<typename Event>
	[[gnu::always_inline]] static void end()
	{
		emit(end_of<Event>{});
	}

	/** @brief Closes the duration of @p Event opened under @p key. */
	template<typename Event>
	[[gnu::always_inline]] static void end(std::uint32_t key)
	{
		emit(keyed_end_of<Event>{key});
	}

	/** @brief A duration of @p Event, closed when it goes out of scope. */
	template<typename Event>
	class [[nodiscard]] scoped
	{
	public:
		scoped(scoped const&) = delete;
		scoped(scoped&&) = delete;
		scoped& operator=(scoped const&) = delete;
		scoped& operator=(scoped&&) = delete;
		[[gnu::always_inline]] ~scoped() { end<Event>(); }

	private:
		[[gnu::always_inline]] explicit scoped(Event const& event) { begin(event); }
		friend tracer;
	};

	/** @brief Opens a duration of @p Event, closed at the end of the scope. */
	template<typename Event>
	[[gnu::always_inline]] static scoped<Event> scope(Event const& event)
	{
		return scoped<Event>{event};
	}

private:
	tracer() = default;
	friend Trace;
};

} // namespace kymo
