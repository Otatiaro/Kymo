/**
 * @file    channels.cpp
 * @brief   Two channels in one image: each event goes to the ring of its
 *          family, and the host reads each ring by its name (channels.py).
 *
 * The tracer routes the application family to app::ring, of 32 slots, and
 * every other family to os::ring, of 16. The image emits events of both,
 * fewer than either ring holds, and reports each through semihosting:
 *   emitted <ring> <index> <type> <field>=<value>...
 * the index coming from the ring the event is routed to. Then it reports
 * where the slots start in a ring, and both rings:
 *   slots <offset of the slots in a ring>
 *   ring <name> <hexadecimal bytes of the ring>
 */
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>

namespace os
{
/// The ring of every family but the application's.
inline kymo::ring<16> ring;
} // namespace os

namespace app
{
/// The ring of the application's events.
inline kymo::ring<32> ring;
} // namespace app

/** @brief Routes the application family to app::ring, every other to os::ring. */
struct trace : kymo::tracer<trace>
{
	static auto& channel(kymo::on<kymo::family::application> /*family*/)
	{
		return app::ring;
	}

	template<kymo::family F>
	static auto& channel(kymo::on<F> /*family*/)
	{
		return os::ring;
	}
};

/// An application event of one slot.
struct mark
{
	std::uint32_t n;
};

/// An application event of two slots.
struct sample
{
	std::uint32_t n;
	std::array<std::uint32_t, 3> words;
};

/// A scheduling event, routed to os::ring.
struct switched
{
	static constexpr kymo::family kymo_family = kymo::family::scheduling;
	std::uint32_t n;
};

namespace
{

static_assert(kymo::slots_for<mark> == 1 && kymo::slots_for<sample> == 2 && kymo::slots_for<switched> == 1);

/// Events of the sequence: app::ring takes 30 slots of its 32, os::ring 10 of its 16.
constexpr std::uint32_t events = 20;

/// Emits @p event, and reports the ring it must reach and its index there.
template<typename Ring, typename Event>
void emitted(char const* ring_name, Ring& ring, Event const& event, char const* fields)
{
	std::uint32_t const index = ring.write_index.load(std::memory_order_relaxed);
	trace::emit(event);
	qemu::write("emitted ");
	qemu::write(ring_name);
	qemu::write(" ");
	qemu::write_unsigned(index);
	qemu::write(" ");
	qemu::write(fields);
	qemu::write_unsigned(event.n);
}

template<typename Ring>
void report(char const* name, Ring const& ring)
{
	constexpr std::array<char, 16> hex{'0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f'};
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast): the ring's bytes
	auto const* const bytes = reinterpret_cast<unsigned char const volatile*>(&ring);
	std::array<char, 3> text{};   // the last one stays the terminator
	qemu::write("ring ");
	qemu::write(name);
	qemu::write(" ");
	for (std::size_t n = 0; n < sizeof(Ring); ++n)
	{
		unsigned char const byte = bytes[n];   // NOLINT(cppcoreguidelines-pro-bounds-pointer-arithmetic): within the ring
		text[0] = hex[byte / 16U];
		text[1] = hex[byte % 16U];
		qemu::write(text.data());
	}
	qemu::write("\n");
}

} // namespace

void test_main()
{
	for (std::uint32_t n = 0; n < events; ++n)
	{
		emitted("app::ring", app::ring, mark{n}, "mark n=");
		qemu::write("\n");
		if (n % 4 == 0)
		{
			emitted("app::ring", app::ring, sample{n, {n + 1, n + 2, n + 3}}, "sample n=");
			qemu::write(" words=[");
			qemu::write_unsigned(n + 1);
			qemu::write(",");
			qemu::write_unsigned(n + 2);
			qemu::write(",");
			qemu::write_unsigned(n + 3);
			qemu::write("]\n");
		}
		if (n % 2 == 0)
		{
			emitted("os::ring", os::ring, switched{n}, "switched n=");
			qemu::write("\n");
		}
	}
	qemu::write("slots ");
	qemu::write_unsigned(offsetof(decltype(os::ring), slots));
	qemu::write("\n");
	report("os::ring", os::ring);
	report("app::ring", app::ring);
	qemu::finish(true);
}
