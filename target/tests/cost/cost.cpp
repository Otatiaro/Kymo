/**
 * @file    cost.cpp
 * @brief   What an emit costs: one function per case, each called once, for
 *          cost.py to count the instructions it executes under QEMU.
 *
 * Built twice: with Kymo, and with KYMO_COST_BASELINE, where every function
 * builds the same event and hands its address to the compiler instead of
 * emitting it. The difference between the two is what the emit adds; the
 * difference between their sections is Kymo's flash and RAM.
 *
 * The cases: an event of one to four slots, the largest each slot count
 * takes, and an event whose family is disabled.
 */
#include "../qemu/qemu.hpp"

#ifndef KYMO_COST_BASELINE
#include <kymo/kymo.hpp>
#endif

#include <cstdint>

namespace
{

struct slots_1
{
	std::uint32_t words[2];
};

struct slots_2
{
	std::uint32_t words[5];
};

struct slots_3
{
	std::uint32_t words[8];
};

struct slots_4
{
	std::uint32_t words[11];
};

struct disabled
{
#ifndef KYMO_COST_BASELINE
	static constexpr kymo::family kymo_family = kymo::family::meta;
#endif
	std::uint32_t words[2];
};

#ifndef KYMO_COST_BASELINE
static_assert(kymo::slots_for<slots_1> == 1, "");
static_assert(kymo::slots_for<slots_2> == 2, "");
static_assert(kymo::slots_for<slots_3> == 3, "");
static_assert(kymo::slots_for<slots_4> == 4, "");
#endif

/** @brief An event whose words all come from @p seed, so none is a constant. */
template<typename Event>
[[gnu::always_inline]] inline Event make(std::uint32_t seed)
{
	Event event;
	for (std::uint32_t& word : event.words)
		word = seed++;
	return event;
}

template<typename Event>
[[gnu::always_inline]] inline void emit(std::uint32_t seed)
{
	Event const event = make<Event>(seed);
#ifdef KYMO_COST_BASELINE
	asm volatile("" : : "r"(&event) : "memory");
#else
	kymo::emit(event);
#endif
}

} // namespace

extern "C" [[gnu::noinline]] void cost_slots_1(std::uint32_t seed) { emit<slots_1>(seed); }
extern "C" [[gnu::noinline]] void cost_slots_2(std::uint32_t seed) { emit<slots_2>(seed); }
extern "C" [[gnu::noinline]] void cost_slots_3(std::uint32_t seed) { emit<slots_3>(seed); }
extern "C" [[gnu::noinline]] void cost_slots_4(std::uint32_t seed) { emit<slots_4>(seed); }
extern "C" [[gnu::noinline]] void cost_disabled(std::uint32_t seed) { emit<disabled>(seed); }

void test_main()
{
#ifndef KYMO_COST_BASELINE
	kymo::disable(kymo::family::meta);
#endif
	cost_slots_1(1);
	cost_slots_2(2);
	cost_slots_3(3);
	cost_slots_4(4);
	cost_disabled(5);
	qemu::finish(true);
}
