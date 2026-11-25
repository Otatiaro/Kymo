/**
 * @file    text.cpp
 * @brief   Events whose types declare a text (kymo_text): what the host
 *          writes for each must be its text with each field's value, as
 *          std::format writes it (text.py).
 *
 * The image reports each type's text, then emits one event of each type and
 * reports the line the host must print for it:
 *   text <type> <kymo_text>
 *   emitted <index> event <index> <cycles> - <type> <text, its fields written>
 * then the ring, taken with nothing writing:
 *   ring <hexadecimal bytes of trace_ring>
 *
 * Built with KYMO_TEST_BAD_TEXT, the image also holds a type whose text names
 * a field it does not have: the host must refuse the image.
 */
#include "../qemu/qemu.hpp"

#include "../test_trace.hpp"

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <iterator>

enum class mode : std::uint8_t
{
	idle,
	run,
};

struct moved
{
	mode from;
	mode to;
	static constexpr char kymo_text[] = "{from} -> {to}";   // NOLINT(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): the host reads it from the DWARF
};

struct sample
{
	std::int32_t delta;
	std::uint16_t raw;
	float gain;
	double volts;
	bool ok;
	char unit;
	static constexpr char kymo_text[] =   // NOLINT(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): the host reads it from the DWARF
	    "delta={delta:+} raw={raw:#06x} gain={gain} volts={volts:.3f} ok={ok} unit={unit}";
};

struct named
{
	std::array<char, 8> label;
	std::int8_t level;
	static constexpr char kymo_text[] = "[{label:>8}] {{level}}={level:+d}";   // NOLINT(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): the host reads it from the DWARF
};

/// No text: the host writes its fields.
struct plain
{
	std::uint32_t x;
};

#ifdef KYMO_TEST_BAD_TEXT
struct wrong
{
	std::int32_t x;
	static constexpr char kymo_text[] = "x={y}";   // NOLINT(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): the host reads it from the DWARF
};
#endif

namespace
{

template<typename Event>
void text(char const* type)
{
	qemu::write("text ");
	qemu::write(type);
	qemu::write(" ");
	qemu::write(std::data(Event::kymo_text));
	qemu::write("\n");
}

/// Emits @p event, then reports where it went and the line the host must print.
template<typename Event>
void emitted(char const* type, Event const& event, char const* written)
{
	std::uint32_t const index = trace_ring.write_index.load(std::memory_order_relaxed);
	trace::emit(event);
	// QEMU has no cycle counter: each event gets its own timestamp.
	std::uint32_t const timestamp = 0x10000000U + (index * 7);
	trace_ring.slots[index % ring_slots].words[0] = timestamp;
	qemu::write("emitted ");
	qemu::write_unsigned(index);
	qemu::write(" event ");
	qemu::write_unsigned(index);
	if constexpr (kymo::slots_for<Event> > 1)
	{
		qemu::write("-");
		qemu::write_unsigned(index + kymo::slots_for<Event> - 1);
	}
	qemu::write(" ");
	qemu::write_unsigned(timestamp);
	qemu::write(" - ");
	qemu::write(type);
	qemu::write(" ");
	qemu::write(written);
	qemu::write("\n");
}

void report(char const* label)
{
	constexpr std::array<char, 16> hex{'0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f'};
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast): the ring's bytes
	auto const* const bytes = reinterpret_cast<unsigned char const volatile*>(&trace_ring);
	std::array<char, (2 * sizeof(trace_ring)) + 1> out{};   // the last one stays the terminator
	for (std::size_t n = 0; n < sizeof(trace_ring); ++n)
	{
		out[2 * n] = hex[bytes[n] / 16U];         // NOLINT(cppcoreguidelines-pro-bounds-pointer-arithmetic): within the ring
		out[(2 * n) + 1] = hex[bytes[n] % 16U];   // NOLINT(cppcoreguidelines-pro-bounds-pointer-arithmetic): within the ring
	}
	qemu::write(label);
	qemu::write(" ");
	qemu::write(out.data());
	qemu::write("\n");
}

} // namespace

void test_main()
{
	text<moved>("moved");
	text<sample>("sample");
	text<named>("named");

	emitted("moved", moved{mode::idle, mode::run}, "idle -> run");
	// NOLINTNEXTLINE(clang-analyzer-optin.core.EnumCastOutOfRange): a value no enumerator names, written as a number
	emitted("moved", moved{mode::run, mode{7}}, "run -> 7");
	emitted("sample", sample{-42, 0xbeef, 0.1F, 3.3, true, 'V'},
	        "delta=-42 raw=0xbeef gain=0.1 volts=3.300 ok=true unit=V");
	emitted("sample", sample{7, 1, 1e10F, -0.0005, false, '~'},
	        "delta=+7 raw=0x0001 gain=1e+10 volts=-0.001 ok=false unit=~");
	emitted("named", named{{'s', 'e', 'n', 's', 'o', 'r', '\0', 'x'}, -3}, "[  sensor] {level}=-3");
	emitted("plain", plain{12}, "x=12");
#ifdef KYMO_TEST_BAD_TEXT
	trace::emit(wrong{1});
#endif

	report("ring");
	qemu::finish(true);
}
