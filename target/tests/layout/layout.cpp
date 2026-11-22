/**
 * @file    layout.cpp
 * @brief   The layout of each event, as the compiler laid it out: what the
 *          host reads from the DWARF must say the same (layout.py).
 *
 * Each event is emitted once, as a program would, then reported through
 * semihosting:
 *   event <id> <qualified name> <size>
 *     field <name> <offset> <size> <type>
 * The ids, offsets and sizes come from the compiler; the names are written
 * here, as the host is expected to print them.
 *
 * The events cover what a layout can hold: padding before an 8-byte member,
 * an enum, a pointer, arrays, a nested structure, a const member, no member
 * at all, and names in the global namespace, a nested namespace, a class and
 * an anonymous namespace.
 */
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

#include <cstddef>
#include <cstdint>

struct plain
{
	std::uint32_t value;
};

namespace app::motor
{

enum class mode : std::uint8_t
{
	idle,
	run,
};

struct step
{
	std::uint8_t phase;
	std::uint64_t at;
	std::int16_t delta;
};

struct mode_changed
{
	mode from;
	mode to;
	void const* source;
	char tag[3];
};

} // namespace app::motor

struct point
{
	std::int16_t x;
	std::int16_t y;
};

struct sensor
{
	struct moved
	{
		point to;
		std::uint16_t samples[2][3];
		bool const valid;
	};
};

namespace
{

struct tick
{
};

} // namespace

namespace
{

template<typename Event>
void event(char const* name)
{
	qemu::write("event ");
	qemu::write_unsigned(kymo::type_id<Event>());
	qemu::write(" ");
	qemu::write(name);
	qemu::write(" ");
	qemu::write_unsigned(sizeof(Event));
	qemu::write("\n");
}

void field(char const* name, std::size_t offset, std::size_t size, char const* type)
{
	qemu::write("  field ");
	qemu::write(name);
	qemu::write(" ");
	qemu::write_unsigned(offset);
	qemu::write(" ");
	qemu::write_unsigned(size);
	qemu::write(" ");
	qemu::write(type);
	qemu::write("\n");
}

int origin;

} // namespace

#define FIELD(Event, member, type) field(#member, offsetof(Event, member), sizeof(Event::member), type)

void test_main()
{
	kymo::emit(plain{1});
	kymo::emit(app::motor::step{2, 3, -4});
	kymo::emit(app::motor::mode_changed{app::motor::mode::idle, app::motor::mode::run, &origin, {'a', 'b', 'c'}});
	kymo::emit(sensor::moved{{5, 6}, {{1, 2, 3}, {4, 5, 6}}, true});
	kymo::emit(tick{});

	event<plain>("plain");
	FIELD(plain, value, "uint32_t");

	event<app::motor::step>("app::motor::step");
	FIELD(app::motor::step, phase, "uint8_t");
	FIELD(app::motor::step, at, "uint64_t");
	FIELD(app::motor::step, delta, "int16_t");

	event<app::motor::mode_changed>("app::motor::mode_changed");
	FIELD(app::motor::mode_changed, from, "mode");
	FIELD(app::motor::mode_changed, to, "mode");
	FIELD(app::motor::mode_changed, source, "void const*");
	FIELD(app::motor::mode_changed, tag, "char[3]");

	event<sensor::moved>("sensor::moved");
	FIELD(sensor::moved, to, "point");
	FIELD(sensor::moved, samples, "uint16_t[2][3]");
	FIELD(sensor::moved, valid, "bool const");

	event<tick>("(anonymous namespace)::tick");

	qemu::finish(true);
}
