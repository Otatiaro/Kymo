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
 * takes, and an event whose family is disabled. Then the application's
 * events, each beside the plain emit of the same payload, which it must cost
 * exactly: an event with a text against slots_1; a duration's begin against
 * slots_1, its end against an empty event; a keyed begin against three
 * words, a keyed end against one (cost.py, SAME).
 */
#include "../qemu/qemu.hpp"

#ifndef KYMO_COST_BASELINE
#include "../test_trace.hpp"
#endif

#include <array>
#include <cstdint>

namespace
{

struct slots_1
{
	std::array<std::uint32_t, 2> words;
};

struct slots_2
{
	std::array<std::uint32_t, 5> words;
};

struct slots_3
{
	std::array<std::uint32_t, 8> words;
};

struct slots_4
{
	std::array<std::uint32_t, 11> words;
};

struct disabled
{
#ifndef KYMO_COST_BASELINE
	static constexpr kymo::family kymo_family = kymo::family::kernel;
#endif
	std::array<std::uint32_t, 2> words;
};

/** @brief slots_1 with a text: the text is in the DWARF only. */
struct texted
{
	std::array<std::uint32_t, 2> words;
	static constexpr char kymo_text[] = "{words}";   // NOLINT(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): the host reads it from the DWARF
};

/** @brief The payload of a duration's end. */
struct empty
{
};

/** @brief The payload of a keyed begin of slots_1: the key and its two words. */
struct words_3
{
	std::array<std::uint32_t, 3> words;
};

/** @brief The payload of a keyed end: the key. */
struct words_1
{
	std::array<std::uint32_t, 1> words;
};

#ifndef KYMO_COST_BASELINE
static_assert(kymo::slots_for<slots_1> == 1);
static_assert(kymo::slots_for<slots_2> == 2);
static_assert(kymo::slots_for<slots_3> == 3);
static_assert(kymo::slots_for<slots_4> == 4);
static_assert(sizeof(kymo::begin_of<slots_1>) == sizeof(slots_1));
static_assert(sizeof(kymo::end_of<slots_1>) == sizeof(empty));
static_assert(sizeof(kymo::keyed_begin_of<slots_1>) == sizeof(words_3));
static_assert(sizeof(kymo::keyed_end_of<slots_1>) == sizeof(words_1));
#endif

/** @brief An event whose words all come from @p seed, so none is a constant. */
template<typename Event>
[[gnu::always_inline]] inline Event make(std::uint32_t seed)
{
	Event event{};
	for (std::uint32_t& word : event.words)
		word = seed++;
	return event;
}

/** @brief Hands @p value to the compiler, as an emit would take it. */
template<typename Value>
[[gnu::always_inline]] inline void keep([[maybe_unused]] Value const& value)
{
	asm volatile("" : : "r"(&value) : "memory");
}

template<typename Event>
[[gnu::always_inline]] inline void emit(std::uint32_t seed)
{
	auto const event = make<Event>(seed);
#ifdef KYMO_COST_BASELINE
	keep(event);
#else
	trace::emit(event);
#endif
}

} // namespace

extern "C" [[gnu::noinline]] void cost_slots_1(std::uint32_t seed) { emit<slots_1>(seed); }
extern "C" [[gnu::noinline]] void cost_slots_2(std::uint32_t seed) { emit<slots_2>(seed); }
extern "C" [[gnu::noinline]] void cost_slots_3(std::uint32_t seed) { emit<slots_3>(seed); }
extern "C" [[gnu::noinline]] void cost_slots_4(std::uint32_t seed) { emit<slots_4>(seed); }
extern "C" [[gnu::noinline]] void cost_disabled(std::uint32_t seed) { emit<disabled>(seed); }
extern "C" [[gnu::noinline]] void cost_text(std::uint32_t seed) { emit<texted>(seed); }
extern "C" [[gnu::noinline]] void cost_words_3(std::uint32_t seed) { emit<words_3>(seed); }
extern "C" [[gnu::noinline]] void cost_words_1(std::uint32_t seed) { emit<words_1>(seed); }

extern "C" [[gnu::noinline]] void cost_empty()
{
#ifdef KYMO_COST_BASELINE
	keep(empty{});
#else
	trace::emit(empty{});
#endif
}

extern "C" [[gnu::noinline]] void cost_begin(std::uint32_t seed)
{
	auto const event = make<slots_1>(seed);
#ifdef KYMO_COST_BASELINE
	keep(event);
#else
	trace::begin(event);
#endif
}

extern "C" [[gnu::noinline]] void cost_end()
{
#ifdef KYMO_COST_BASELINE
	keep(empty{});
#else
	trace::end<slots_1>();
#endif
}

extern "C" [[gnu::noinline]] void cost_keyed_begin(std::uint32_t seed)
{
#ifdef KYMO_COST_BASELINE
	keep(make<words_3>(seed));
#else
	trace::begin(make<slots_1>(seed + 1), seed);
#endif
}

extern "C" [[gnu::noinline]] void cost_keyed_end(std::uint32_t seed)
{
#ifdef KYMO_COST_BASELINE
	keep(make<words_1>(seed));
#else
	trace::end<slots_1>(seed);
#endif
}

void test_main()
{
#ifndef KYMO_COST_BASELINE
	kymo::disable(kymo::family::kernel);
#endif
	cost_slots_1(1);
	cost_slots_2(2);
	cost_slots_3(3);
	cost_slots_4(4);
	cost_disabled(5);
	cost_text(6);
	cost_begin(6);
	cost_empty();
	cost_end();
	cost_words_3(7);
	cost_keyed_begin(7);
	cost_words_1(8);
	cost_keyed_end(8);
	qemu::finish(true);
}
