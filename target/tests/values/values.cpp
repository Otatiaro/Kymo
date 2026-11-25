/**
 * @file    values.cpp
 * @brief   Numeric fields of application events, read by the host as curves
 *          (values.py).
 *
 * Run on its own, the image reports each event it emits and each numeric
 * field's bits, as the host must read them:
 *   emit <index>
 *   value <event>.<field> <reading> <bits in hex>
 * where the reading is i8, i32, i64, u16, u32, f32 or f64. A bool, a char and
 * the fields of kymo's own events are not values: the image does not report
 * them. Recorded, the same image gives the capture the host reads.
 */
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

#include <array>
#include <cstdint>
#include <cstring>
#include <limits>

namespace axis
{

/// A setpoint of each width and sign, and a flag.
struct setpoint
{
	std::int32_t x;
	std::uint32_t y;
	std::int64_t z;
	std::int8_t w;
	bool on;
};

} // namespace axis

/// Floating-point values, one of them NaN.
struct gauge
{
	float f;
	double d;
};

/// A character, not a value, beside a value.
struct label
{
	char c;
	std::uint16_t n;
};

/// A duration: its begin is kymo's event, its field no value.
struct batch
{
	std::uint32_t id;
};

namespace
{

/// The ring the host drains, large enough to hold every event.
kymo::ring<2048> values_ring;

/// The test's clock: each event 10 cycles after the previous one.
std::uint32_t now;

/** @brief Writes to values_ring, each event at the test's clock. */
struct clocked
{
	template<typename Event>
	static void write(std::uint32_t type, std::uint32_t /*timestamp*/, Event const& event)
	{
		now += 10;
		values_ring.write(type, now, event);
	}
};

clocked clocked_channel;

} // namespace

/** @brief Routes every family to the test's clocked channel. */
struct trace : kymo::tracer<trace>
{
	template<kymo::family F>
	static auto& channel(kymo::on<F> /*family*/)
	{
		return clocked_channel;
	}
};

namespace
{

/// The samples of the half wave.
constexpr std::int64_t samples = 48;
/// Its peak.
constexpr std::int64_t peak = 19998;

/// The half wave at @p n: 0 at both ends, the peak at the middle.
constexpr std::int64_t wave(std::int64_t n)
{
	return 4 * peak * n * (samples - n) / (samples * samples);
}

/// The setpoints, computed by the compiler: the image links no 64-bit or
/// floating-point arithmetic.
constexpr auto setpoints = [] {
	std::array<axis::setpoint, samples + 1> points{};
	for (std::int64_t n = 0; n <= samples; ++n)
	{
		std::int64_t const s = wave(n);
		points[static_cast<std::size_t>(n)] = axis::setpoint{
			static_cast<std::int32_t>(s - 10000),
			0xFFFF0000U + static_cast<std::uint32_t>(s),
			s * -1000000007LL,
			static_cast<std::int8_t>((s % 256) - 128),
			(n % 2) == 0,
		};
	}
	return points;
}();

/// A gauge every fourth setpoint, exact in binary.
constexpr auto gauges = [] {
	std::array<gauge, samples / 4 + 1> readings{};
	for (std::size_t n = 0; n < readings.size(); ++n)
		readings[n] = gauge{static_cast<float>(n) - 3.0F, static_cast<double>(n) * -2.0 + 7.0};
	return readings;
}();

void write_hex(std::uint64_t bits)
{
	std::array<char, 19> text{'0', 'x'};   // the last one stays the terminator
	for (std::size_t digit = 0; digit < 16; ++digit)
	{
		auto const nibble = static_cast<unsigned>(bits >> (4 * (15 - digit))) & 0xFU;
		text[2 + digit] = static_cast<char>(nibble < 10 ? '0' + nibble : 'a' + nibble - 10);
	}
	qemu::write(text.data());
}

/// Reports the index the next event goes to.
void emitted()
{
	qemu::write("emit ");
	qemu::write_unsigned(values_ring.write_index.load(std::memory_order_relaxed));
	qemu::write("\n");
}

void value(char const* curve, char const* reading, std::uint64_t bits)
{
	qemu::write("value ");
	qemu::write(curve);
	qemu::write(" ");
	qemu::write(reading);
	qemu::write(" ");
	write_hex(bits);
	qemu::write("\n");
}

void emit(axis::setpoint const& point)
{
	emitted();
	trace::emit(point);
	value("axis::setpoint.x", "i32", static_cast<std::uint32_t>(point.x));
	value("axis::setpoint.y", "u32", point.y);
	value("axis::setpoint.z", "i64", static_cast<std::uint64_t>(point.z));
	value("axis::setpoint.w", "i8", static_cast<std::uint8_t>(point.w));
}

template<typename Bits, typename Float>
Bits bits_of(Float value)
{
	static_assert(sizeof(Bits) == sizeof(Float));
	Bits bits{};
	std::memcpy(&bits, &value, sizeof bits);
	return bits;
}

void emit(gauge const& reading)
{
	emitted();
	trace::emit(reading);
	value("gauge.f", "f32", bits_of<std::uint32_t>(reading.f));
	value("gauge.d", "f64", bits_of<std::uint64_t>(reading.d));
}

void emit(label const& text)
{
	emitted();
	trace::emit(text);
	value("label.n", "u16", text.n);
}

} // namespace

void test_main()
{
	for (std::size_t n = 0; n < setpoints.size(); ++n)
	{
		emit(setpoints[n]);
		if (n % 4 == 0)
			emit(gauges[n / 4]);
	}
	emit(gauge{std::numeric_limits<float>::quiet_NaN(), 1.0});
	emit(label{'k', 7});
	emit(label{'y', 65535});
	trace::begin(batch{42});
	trace::end<batch>();

	qemu::finish(true);
}
