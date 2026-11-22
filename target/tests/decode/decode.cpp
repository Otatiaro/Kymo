/**
 * @file    decode.cpp
 * @brief   Events written to the ring, and copies of the ring: what the host
 *          decodes from a copy must be what was written (decode.py).
 *
 * The image emits events of one to four slots, enough to run around the
 * ring several times and through its guard zone, and reports each through
 * semihosting:
 *   emitted <index> <slots> event <index> <timestamp> <type> <field>=<value>...
 * The index comes from the ring; the values are written here, as the host is
 * expected to print them. Then it reports two copies of the ring:
 *   ring <hexadecimal bytes of kymo::trace_ring>
 * taken with nothing writing, as from a stopped target; and
 *   copy
 *   copied <slot>      (after each slot, and the events emitted meanwhile)
 *   live <hexadecimal bytes>
 * taken slot by slot while events keep coming, as a probe reads a running
 * target: the write index first, then each slot in address order.
 */
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

#include <cstddef>
#include <cstdint>

enum class level : std::int8_t
{
	low = -1,
	mid,
	high,
};

struct tiny
{
	bool on;
	level grade;
};

struct pair
{
	std::int32_t x;
	std::uint32_t y;
};

struct reading
{
	level grade;
	std::int16_t values[3];
	void const* from;
	std::uint8_t id;
};

struct point
{
	std::int16_t x;
	std::int16_t y;
};

struct span
{
	std::uint32_t words[6];
};

struct block
{
	point at;
	std::uint32_t words[10];
};

namespace
{

/// Events emitted before the copies, and while the live copy goes on.
constexpr std::uint32_t before_copies = 30;
constexpr std::uint32_t during_copy = 12;
/// The slot after which the live copy starts emitting, one event per slot.
constexpr std::size_t first_emitting_slot = 7;

void write_signed(std::int32_t value)
{
	if (value < 0)
		qemu::write("-");
	qemu::write_unsigned(value < 0 ? 0u - static_cast<std::uint32_t>(value) : static_cast<std::uint32_t>(value));
}

void write_list(std::uint32_t const* words, std::size_t count)
{
	qemu::write("[");
	for (std::size_t n = 0; n < count; ++n)
	{
		if (n > 0)
			qemu::write(",");
		qemu::write_unsigned(words[n]);
	}
	qemu::write("]");
}

void write_grade(level grade)
{
	switch (grade)
	{
	case level::low:
		qemu::write("low");
		return;
	case level::mid:
		qemu::write("mid");
		return;
	case level::high:
		qemu::write("high");
		return;
	}
	qemu::write_unsigned(static_cast<std::uint8_t>(grade));
}

/// Emits @p event, then reports where it went and what the host must read.
template<typename Event, typename Fields>
void emitted(char const* type, Event const& event, Fields fields)
{
	std::uint32_t const index = kymo::trace_ring.write_index.load(std::memory_order_relaxed);
	kymo::emit(event);
	// QEMU has no cycle counter: every timestamp reads 0. Each event gets
	// its own, for the host to show it read the right word.
	std::uint32_t const timestamp = 0x10000000u + index * 7;
	kymo::trace_ring.slots[index % KYMO_RING_SLOTS].words[0] = timestamp;
	qemu::write("emitted ");
	qemu::write_unsigned(index);
	qemu::write(" ");
	qemu::write_unsigned(kymo::slots_for<Event>);
	qemu::write(" event ");
	qemu::write_unsigned(index);
	qemu::write(" ");
	qemu::write_unsigned(timestamp);
	qemu::write(" ");
	qemu::write(type);
	fields(event);
	qemu::write("\n");
}

int origin;

/// Event @p n of the sequence: the five types in turn.
void emit_number(std::uint32_t n)
{
	switch (n % 5)
	{
	case 0:
		emitted("tiny", tiny{n % 2 == 0, static_cast<level>(static_cast<int>(n / 5 % 3) - 1)}, [](tiny const& event) {
			qemu::write(event.on ? " on=true grade=" : " on=false grade=");
			write_grade(event.grade);
		});
		break;
	case 1:
		emitted("pair", pair{-static_cast<std::int32_t>(n) * 1000, 0xfffffff0u + n}, [](pair const& event) {
			qemu::write(" x=");
			write_signed(event.x);
			qemu::write(" y=");
			qemu::write_unsigned(event.y);
		});
		break;
	case 2:
	{
		// Every third reading has a grade no enumerator names.
		auto const grade = static_cast<level>(n % 3 == 0 ? 5 : static_cast<int>(n % 2) - 1);
		auto const value = static_cast<std::int16_t>(n * 1000);
		emitted("reading",
		        reading{grade, {value, static_cast<std::int16_t>(-value), -32768}, &origin, static_cast<std::uint8_t>(n)},
		        [](reading const& event) {
			        qemu::write(" grade=");
			        write_grade(event.grade);
			        qemu::write(" values=[");
			        for (int k = 0; k < 3; ++k)
			        {
				        if (k > 0)
					        qemu::write(",");
				        write_signed(event.values[k]);
			        }
			        qemu::write("] from=(anonymous_namespace)::origin");
			        qemu::write(" id=");
			        qemu::write_unsigned(event.id);
		        });
		break;
	}
	case 3:
	{
		span event{};
		for (std::uint32_t k = 0; k < 6; ++k)
			event.words[k] = n * 100 + k;
		emitted("span", event, [](span const& event) {
			qemu::write(" words=");
			write_list(event.words, 6);
		});
		break;
	}
	default:
	{
		block event{{static_cast<std::int16_t>(n), static_cast<std::int16_t>(-static_cast<int>(n))}, {}};
		for (std::uint32_t k = 0; k < 10; ++k)
			event.words[k] = 0x01010101u * k + n;
		emitted("block", event, [](block const& event) {
			qemu::write(" at={x=");
			write_signed(event.at.x);
			qemu::write(" y=");
			write_signed(event.at.y);
			qemu::write("} words=");
			write_list(event.words, 10);
		});
		break;
	}
	}
}

using ring_bytes = unsigned char[sizeof(kymo::trace_ring)];

void copy(unsigned char* to, std::size_t offset, std::size_t size)
{
	auto const* const from = reinterpret_cast<unsigned char const volatile*>(&kymo::trace_ring);
	for (std::size_t n = offset; n < offset + size; ++n)
		to[n] = from[n];
}

void report(char const* label, ring_bytes const& bytes)
{
	char text[2 * sizeof(ring_bytes) + 1];
	for (std::size_t n = 0; n < sizeof(ring_bytes); ++n)
	{
		text[2 * n] = "0123456789abcdef"[bytes[n] >> 4];
		text[2 * n + 1] = "0123456789abcdef"[bytes[n] & 0xf];
	}
	text[2 * sizeof(ring_bytes)] = '\0';
	qemu::write(label);
	qemu::write(" ");
	qemu::write(text);
	qemu::write("\n");
}

ring_bytes stopped;
ring_bytes live;

} // namespace

void test_main()
{
	static_assert(kymo::slots_for<tiny> == 1 && kymo::slots_for<pair> == 1 && kymo::slots_for<reading> == 2
	              && kymo::slots_for<span> == 3 && kymo::slots_for<block> == 4);
	constexpr std::size_t slots = sizeof(kymo::trace_ring.slots) / sizeof(kymo::trace_ring.slots[0]);
	auto const offset = [](void const* member) {
		return static_cast<std::size_t>(reinterpret_cast<unsigned char const*>(member)
		                                - reinterpret_cast<unsigned char const*>(&kymo::trace_ring));
	};

	std::uint32_t n = 0;
	for (; n < before_copies; ++n)
		emit_number(n);

	copy(stopped, 0, sizeof(ring_bytes));
	report("ring", stopped);

	qemu::write("copy\n");
	copy(live, offset(&kymo::trace_ring.write_index), sizeof(kymo::trace_ring.write_index));
	for (std::size_t slot = 0; slot < slots; ++slot)
	{
		copy(live, offset(&kymo::trace_ring.slots[slot]), sizeof(kymo::trace_ring.slots[slot]));
		qemu::write("copied ");
		qemu::write_unsigned(slot);
		qemu::write("\n");
		if (slot >= first_emitting_slot && n < before_copies + during_copy)
			emit_number(n++);
	}
	report("live", live);

	qemu::finish(true);
}
