/**
 * @file    dwt.cpp
 * @brief   The comparators kymo::dwt takes: the least capable free one that
 *          sends data values, checked at compile time against the registers
 *          of real chips; and, under QEMU, which models no DWT comparator,
 *          start() and watch() refusing, start() publishing no baud rate.
 *
 * The data trace itself runs on hardware only: the NUCLEO-H563ZI's board
 * test (target/opsy/tests/board).
 */
#include "../qemu/qemu.hpp"

#include <kymo/dwt.hpp>
#include <kymo/ring.hpp>

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>

namespace
{

/** @brief DWT_CTRL with @p comparators comparators, trace packets supported. */
constexpr std::uint32_t ctrl_with(std::uint32_t comparators)
{
	return comparators << 28;
}

/** @brief The comparator least_capable_free() chooses among @p functions. */
template<std::size_t N>
constexpr int choose(std::uint32_t ctrl, std::array<std::uint32_t, N> const& functions)
{
	return kymo::dwt::detail::least_capable_free(ctrl, [&](unsigned n) { return functions.at(n); });
}

constexpr std::uint32_t in_use = 0x82D; ///< a data value on write, 4 bytes

#if KYMO_DWT_ARMV8M
// The STM32H563's four comparators, DWT_FUNCTIONn.ID read on the board:
// cycle counter + instruction + data address; instruction and data address
// with limits; instruction + data address; all, data value matching included.
constexpr std::array<std::uint32_t, 4> h563{0b01011U << 27, 0b11010U << 27, 0b01010U << 27, 0b11110U << 27};
static_assert(choose(ctrl_with(4), h563) == 2, "the anchor goes on the comparator with the fewest capabilities");

constexpr auto h563_anchored = [] {
	auto functions = h563;
	functions[2] |= in_use;
	return functions;
}();
static_assert(choose(ctrl_with(4), h563_anchored) == 1, "a tie goes to the higher number, comparator 0 kept for the cycle counter");

constexpr auto h563_full = [] {
	auto functions = h563_anchored;
	functions[0] |= in_use;
	functions[1] |= in_use;
	functions[3] |= in_use;
	return functions;
}();
static_assert(choose(ctrl_with(4), h563_full) == -1, "no free comparator");

constexpr std::array<std::uint32_t, 6> beyond_four{
	0b11110U << 27, 0b11110U << 27, 0b11110U << 27, 0b11110U << 27, 0b01000U << 27, 0b01000U << 27,
};
static_assert(choose(ctrl_with(6), beyond_four) == 3, "only the first four comparators send data values");

constexpr std::array<std::uint32_t, 2> without_value{0b00000U << 27, 0b11110U << 27};
static_assert(choose(ctrl_with(2), without_value) == 1, "a comparator without Data Address With Value is left");
static_assert(choose(ctrl_with(4) | (1U << 27), h563) == -1, "NOTRCPKT: no trace packets");
#else
// ARMv7-M does not tell a comparator's capabilities: the last free one.
constexpr std::array<std::uint32_t, 4> m4{0, 0, 0, 0};
static_assert(choose(ctrl_with(4), m4) == 3, "the anchor goes on the last comparator");

constexpr std::array<std::uint32_t, 4> m4_anchored{0, 0, 0, in_use & 0xFU};
static_assert(choose(ctrl_with(4), m4_anchored) == 2, "the next variable on the last free comparator");
static_assert(choose(ctrl_with(4) | (1U << 27), m4) == -1, "NOTRCPKT: no trace packets");
#endif
static_assert(choose(ctrl_with(0), std::array<std::uint32_t, 0>{}) == -1, "no comparator");

// What start() frees after a reset: what sends trace packets, not a debugger's
// watchpoint or trigger.
using kymo::dwt::detail::sends_trace;
static_assert(!sends_trace(0), "a disabled comparator");
#if KYMO_DWT_ARMV8M
static_assert(sends_trace(0x82D), "a data value on write");
static_assert(sends_trace(0x03D), "a data address on write");
static_assert(!sends_trace(0x01D), "a watchpoint: a debug event");
static_assert(!sends_trace(0x00D), "a trigger for another comparator");
static_assert(!sends_trace(0x020), "a trace action with matching disabled");
#else
static_assert(sends_trace(0b1101) && sends_trace(0b1111) && sends_trace(0b1100), "data values");
static_assert(sends_trace(0b0001) && sends_trace(0b0010) && sends_trace(0b0011), "the PC and data values");
static_assert(!sends_trace(0b0101) && !sends_trace(0b0110) && !sends_trace(0b0111), "watchpoints");
static_assert(!sends_trace(0b0100) && !sends_trace(0b1000) && !sends_trace(0b1011), "PC watchpoint and CMPMATCH");
#endif

kymo::ring<16> dwt_ring;
std::uint32_t watched;

[[noreturn]] void fail(char const* what)
{
	qemu::write("FAIL: ");
	qemu::write(what);
	qemu::write("\n");
	qemu::finish(false);
}

} // namespace

/// What start() and watch() cost, each out of line for cost measures: their sizes in the image.
[[gnu::noinline]] bool dwt_start()
{
	return kymo::dwt::start(dwt_ring, {8'000'000, 2'000'000});
}

[[gnu::noinline]] bool dwt_watch()
{
	return kymo::dwt::watch(watched);
}

[[gnu::noinline]] void dwt_exceptions()
{
	kymo::dwt::exceptions();
}

void test_main()
{
	if (dwt_start())
		fail("start() took a comparator QEMU does not model");
	if (kymo::dwt::swo_baud.load(std::memory_order_relaxed) != 0)
		fail("start() published a baud rate without an anchor");
	if (dwt_watch())
		fail("watch() took a comparator QEMU does not model");
	dwt_exceptions();
	qemu::finish(true);
}
