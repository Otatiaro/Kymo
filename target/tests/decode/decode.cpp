/**
 * @file    decode.cpp
 * @brief   Events written to the ring, and copies of the ring: what the host
 *          decodes from a copy must be what was written (decode.py).
 *
 * The image emits events of one to four slots, enough to run around the
 * ring several times and across its end, and reports each through
 * semihosting:
 *   emitted <index> <slots> event <index> <cycles> - <type> <field>=<value>...
 * The index comes from the ring; the values are written here, as the host is
 * expected to print them. The timestamps only grow, by less than a turn, so
 * their cycles are the timestamps themselves; with no frequency, no seconds. Then it reports two copies of the ring:
 *   ring <hexadecimal bytes of trace_ring>
 * taken with nothing writing, as from a stopped target; and
 *   copy
 *   copied <slot>      (after each slot, and the events emitted meanwhile)
 *   live <hexadecimal bytes>
 * taken slot by slot while events keep coming, as a probe reads a running
 * target: the write index first, then each slot in address order.
 */
#include "../qemu/qemu.hpp"

#include "../test_trace.hpp"

#include <array>
#include <atomic>
#include <chrono>
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
	std::chrono::time_point<std::chrono::steady_clock, std::chrono::duration<std::int16_t, std::milli>> at;
};

struct pair
{
	std::int32_t x;
	std::uint32_t y;
};

struct reading
{
	level grade;
	// NOLINTNEXTLINE(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): the host decodes C arrays as well as std::array
	std::int16_t values[3];
	void const* from;
	std::uint8_t id;
	std::chrono::duration<std::int16_t, std::micro> lasted;
};

struct point
{
	std::int16_t x;
	std::int16_t y;
};

struct span
{
	std::array<std::uint32_t, 6> words;
};

struct block
{
	point at;
	std::array<std::uint32_t, 10> words;
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
	qemu::write_unsigned(value < 0 ? 0U - static_cast<std::uint32_t>(value) : static_cast<std::uint32_t>(value));
}

template<std::size_t Count>
void write_list(std::array<std::uint32_t, Count> const& words)
{
	qemu::write("[");
	for (std::size_t n = 0; n < Count; ++n)
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
	std::uint32_t const index = trace_ring.write_index.load(std::memory_order_relaxed);
	trace::emit(event);
	// QEMU has no cycle counter: every timestamp reads 0. Each event gets
	// its own, for the host to show it read the right word.
	std::uint32_t const timestamp = 0x10000000U + (index * 7);
	trace_ring.slots[index % ring_slots].words[0] = timestamp;
	qemu::write("emitted ");
	qemu::write_unsigned(index);
	qemu::write(" ");
	qemu::write_unsigned(kymo::slots_for<Event>);
	qemu::write(" event ");
	qemu::write_unsigned(index);
	qemu::write(" ");
	qemu::write_unsigned(timestamp);
	qemu::write(" - ");
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
	{
		using milliseconds = std::chrono::duration<std::int16_t, std::milli>;
		// Negative every other one: the count is signed.
		auto const magnitude = static_cast<int>(n * 7);
		auto const at = static_cast<std::int16_t>(n % 2 == 0 ? magnitude : -magnitude);
		tiny const event{n % 2 == 0, static_cast<level>(static_cast<int>(n / 5 % 3) - 1), decltype(tiny::at){milliseconds{at}}};
		emitted("tiny", event, [](tiny const& event) {
			qemu::write(event.on ? " on=true grade=" : " on=false grade=");
			write_grade(event.grade);
			qemu::write(" at=");
			write_signed(event.at.time_since_epoch().count());
			qemu::write("ms");
		});
		break;
	}
	case 1:
		emitted("pair", pair{-static_cast<std::int32_t>(n) * 1000, 0xfffffff0U + n}, [](pair const& event) {
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
		        reading{grade,
		                {value, static_cast<std::int16_t>(-value), -32768},
		                &origin,
		                static_cast<std::uint8_t>(n),
		                std::chrono::duration<std::int16_t, std::micro>{static_cast<std::int16_t>(n * 3)}},
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
			        qemu::write("] from=(anonymous namespace)::origin");
			        qemu::write(" id=");
			        qemu::write_unsigned(event.id);
			        qemu::write(" lasted=");
			        write_signed(event.lasted.count());
			        qemu::write("us");
		        });
		break;
	}
	case 3:
	{
		span event{};
		for (std::uint32_t k = 0; k < 6; ++k)
			event.words[k] = (n * 100) + k;
		emitted("span", event, [](span const& event) {
			qemu::write(" words=");
			write_list(event.words);
		});
		break;
	}
	default:
	{
		block event{{static_cast<std::int16_t>(n), static_cast<std::int16_t>(-static_cast<int>(n))}, {}};
		for (std::uint32_t k = 0; k < 10; ++k)
			event.words[k] = (0x01010101U * k) + n;
		emitted("block", event, [](block const& event) {
			qemu::write(" at={x=");
			write_signed(event.at.x);
			qemu::write(" y=");
			write_signed(event.at.y);
			qemu::write("} words=");
			write_list(event.words);
		});
		break;
	}
	}
}

using ring_type = decltype(trace_ring);
using ring_bytes = std::array<unsigned char, sizeof(ring_type)>;

/// Copies bytes @p offset to @p offset + @p size of the ring, as the host reads them.
void copy(ring_bytes& out, std::size_t offset, std::size_t size)
{
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast): the ring's bytes
	auto const* const ring = reinterpret_cast<unsigned char const volatile*>(&trace_ring);
	for (std::size_t n = offset; n < offset + size; ++n)
		out[n] = ring[n];   // NOLINT(cppcoreguidelines-pro-bounds-pointer-arithmetic): within the ring
}

void report(char const* label, ring_bytes const& bytes)
{
	constexpr std::array<char, 16> hex{'0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f'};
	std::array<char, (2 * sizeof(ring_bytes)) + 1> text{};   // the last one stays the terminator
	for (std::size_t n = 0; n < bytes.size(); ++n)
	{
		text[2 * n] = hex[bytes[n] / 16U];
		text[(2 * n) + 1] = hex[bytes[n] % 16U];
	}
	qemu::write(label);
	qemu::write(" ");
	qemu::write(text.data());
	qemu::write("\n");
}

ring_bytes stopped{};
ring_bytes live{};

} // namespace

void test_main()
{
	static_assert(kymo::slots_for<tiny> == 1 && kymo::slots_for<pair> == 1 && kymo::slots_for<reading> == 2
	              && kymo::slots_for<span> == 3 && kymo::slots_for<block> == 4);
	constexpr std::size_t slots = trace_ring.slots.size();

	std::uint32_t n = 0;
	for (; n < before_copies; ++n)
		emit_number(n);

	copy(stopped, 0, stopped.size());
	report("ring", stopped);

	qemu::write("copy\n");
	copy(live, offsetof(ring_type, write_index), sizeof(trace_ring.write_index));
	for (std::size_t slot = 0; slot < slots; ++slot)
	{
		copy(live, offsetof(ring_type, slots) + (slot * sizeof(kymo::slot)), sizeof(kymo::slot));
		qemu::write("copied ");
		qemu::write_unsigned(slot);
		qemu::write("\n");
		if (slot >= first_emitting_slot && n < before_copies + during_copy)
			emit_number(n++);
	}
	report("live", live);

	qemu::finish(true);
}
