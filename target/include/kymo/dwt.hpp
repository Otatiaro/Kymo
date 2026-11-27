/**
 * @file    dwt.hpp
 * @brief   Variables watched by the DWT: each write to one is sent on SWO,
 *          its value and a local timestamp, without code where it is written;
 *          and, optionally, every exception's entry and exit, and a periodic
 *          tick.
 *
 * The program configures the chip's SWO pin and trace clock, then calls
 * start(), which configures the ITM, the TPIU (asynchronous NRZ at the given
 * baud rate) and the time anchor, and watch() for each variable:
 *
 *     kymo::ring<1024> os;
 *     std::uint32_t speed;
 *
 *     kymo::dwt::start(os, {8'000'000, 2'000'000}); // trace clock, baud rate
 *     kymo::dwt::watch(speed);
 *     kymo::dwt::exceptions();                      // optional: every exception
 *     kymo::dwt::ticks<16'384>();                   // optional: a tick every 16384 cycles
 *
 * **The anchor.** start() puts a comparator on the timestamp word of the
 * ring's slot 0: each event that starts at slot 0, at most once a lap, sends
 * its timestamp on SWO, a point both streams share exactly. The host dates
 * the samples between two anchors by the ITM's local timestamps, and checks
 * that their sum matches the anchors' difference. **This comparator must stay
 * as start() configured it for as long as the trace runs**: a debugger or a
 * program that takes it over cuts the synchronisation, and no sample can be
 * dated any more. It is the least capable comparator able to send data
 * values, so that those which match a value or the cycle counter stay free:
 * chosen by DWT_FUNCTIONn.ID on ARMv8-M, the last one on ARMv7-M, which does
 * not tell.
 *
 * A comparator keeps its function across a system reset, so start() first
 * frees every comparator that sends trace packets: SWO is Kymo's channel, and
 * such a comparator is a leftover of the program that ran before the reset,
 * possibly another image. A watchpoint or a trigger, which a debugger sets,
 * is in use: start() and watch() leave it.
 * The host reads every comparator at the start of a capture, through the
 * debug port, and names the addresses they watch from the ELF.
 *
 * Nothing here is in the path of an emit.
 */
#pragma once

#include <kymo/ring.hpp>

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <initializer_list>
#include <type_traits>

// The architecture selects registers and code paths in #if: macros, not constants.
#if defined(__ARM_ARCH_8M_MAIN__) || defined(__ARM_ARCH_8_1M_MAIN__)
#define KYMO_DWT_ARMV8M 1 // NOLINT(cppcoreguidelines-macro-usage)
#elif defined(__ARM_ARCH_7M__) || defined(__ARM_ARCH_7EM__)
#define KYMO_DWT_ARMV8M 0 // NOLINT(cppcoreguidelines-macro-usage)
#else
#error "kymo/dwt.hpp needs ARMv7-M or ARMv8-M Mainline: the DWT data trace"
#endif

namespace kymo::dwt
{

/** @brief The SWO's configuration: the TPIU's input clock and the line's baud rate. */
struct swo
{
	std::uint32_t trace_clock; ///< Hz, the TPIU's TRACECLKIN
	std::uint32_t baud;        ///< bit/s; trace_clock / baud is a whole number
};

/// The baud rate start() set, for the host to read through the debug port; 0 before.
// NOLINTNEXTLINE(cppcoreguidelines-avoid-non-const-global-variables): written by the program, read by the host
inline std::atomic<std::uint32_t> swo_baud{0};

namespace detail
{

constexpr std::uintptr_t demcr = 0xE000EDFC;
constexpr std::uint32_t demcr_trcena = 1U << 24;

constexpr std::uintptr_t itm_tcr = 0xE0000E80;
constexpr std::uintptr_t itm_lar = 0xE0000FB0;
constexpr std::uint32_t itm_unlock = 0xC5ACCE55;
constexpr std::uint32_t itm_itmena = 1U << 0;
constexpr std::uint32_t itm_tsena = 1U << 1;
constexpr std::uint32_t itm_txena = 1U << 3;
constexpr std::uint32_t itm_busy = 1U << 23;

constexpr std::uintptr_t tpiu_cspsr = 0xE0040004;
constexpr std::uintptr_t tpiu_acpr = 0xE0040010;
constexpr std::uintptr_t tpiu_sppr = 0xE00400F0;
constexpr std::uintptr_t tpiu_ffcr = 0xE0040304;
constexpr std::uint32_t tpiu_nrz = 2;
constexpr std::uint32_t tpiu_formatter_off = 0x100; ///< TrigIn kept, EnFCont cleared

constexpr std::uintptr_t dwt_ctrl = 0xE0001000;
constexpr std::uint32_t dwt_cyccntena = 1U << 0;
constexpr std::uint32_t dwt_exctrcena = 1U << 16;
constexpr std::uint32_t dwt_cycevtena = 1U << 22;
constexpr std::uint32_t dwt_cyctap = 1U << 9; ///< POSTCNT counts CYCCNT's bit 10, not bit 6
constexpr unsigned dwt_postinit_shift = 5;
constexpr unsigned dwt_postpreset_shift = 1;
constexpr std::uint32_t dwt_post_mask = 0xF;
constexpr std::uint32_t cyccnt_bit_6 = 1U << 6;   ///< POSTCNT's count, CYCTAP clear
constexpr std::uint32_t cyccnt_bit_10 = 1U << 10; ///< POSTCNT's count, CYCTAP set
constexpr std::uint32_t dwt_tick_fields =
	dwt_cycevtena | dwt_cyctap | (dwt_post_mask << dwt_postinit_shift) | (dwt_post_mask << dwt_postpreset_shift);
constexpr std::uint32_t dwt_notrcpkt = 1U << 27;
constexpr unsigned dwt_numcomp_shift = 28;

constexpr std::uintptr_t dwt_comp0 = 0xE0001020;
constexpr std::uintptr_t comparator_stride = 16;
constexpr std::uintptr_t mask_offset = 4;     ///< DWT_MASKn, ARMv7-M only
constexpr std::uintptr_t function_offset = 8; ///< DWT_FUNCTIONn
constexpr std::uint32_t match_mask = 0xF;     ///< MATCH (ARMv8-M), FUNCTION (ARMv7-M): 0 is disabled

#if KYMO_DWT_ARMV8M
constexpr unsigned id_shift = 27;
constexpr std::uint32_t id_with_value = 0b01000; ///< ID bit: Data Address With Value
constexpr unsigned with_value_comparators = 4;   ///< only the first four send data values
constexpr std::uint32_t match_write_with_value = 0b1101;
constexpr unsigned action_shift = 4;
constexpr std::uint32_t action_mask = 0b11;
constexpr std::uint32_t action_data_value_packet = 0b10;
constexpr std::uint32_t action_data_value = action_data_value_packet << action_shift;
constexpr unsigned datavsize_shift = 10;
#else
constexpr std::uint32_t function_write_value = 0b1101; ///< data value packet on write, EMITRANGE 0
#endif

/**
 * @brief DWT_CTRL's CYCEVTENA, CYCTAP, POSTINIT and POSTPRESET for a tick
 *        every @p cycles cycles; 0 when no tap and count give it.
 */
constexpr std::uint32_t tick_fields(std::uint32_t cycles)
{
	for (std::uint32_t const tap : {cyccnt_bit_6, cyccnt_bit_10})
		if (cycles % tap == 0 && cycles / tap >= 1 && cycles / tap <= dwt_post_mask + 1)
		{
			std::uint32_t const post = (cycles / tap) - 1;
			return dwt_cycevtena | (tap == cyccnt_bit_6 ? 0 : dwt_cyctap) | (post << dwt_postinit_shift) |
			       (post << dwt_postpreset_shift);
		}
	return 0;
}

/** @brief The register at @p address. */
inline std::uint32_t volatile& reg(std::uintptr_t address)
{
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): a register at a fixed address
	return *reinterpret_cast<std::uint32_t volatile*>(address);
}

/** @brief The number of bits set in @p value. */
constexpr int ones(std::uint32_t value)
{
	int count = 0;
	for (; value != 0; value &= value - 1)
		++count;
	return count;
}

/** @brief log2 of @p size, a power of two. */
constexpr std::uint32_t log2(std::size_t size)
{
	std::uint32_t log = 0;
	for (; size > 1; size >>= 1)
		++log;
	return log;
}

inline std::uintptr_t comparator(unsigned n)
{
	return dwt_comp0 + (comparator_stride * n);
}

/**
 * @brief Whether DWT_FUNCTIONn @p function sends trace packets: data values,
 *        data addresses or the PC, Kymo's channel, against a watchpoint or a
 *        trigger, which a debugger sets.
 */
constexpr bool sends_trace(std::uint32_t function)
{
	std::uint32_t const match = function & match_mask;
#if KYMO_DWT_ARMV8M
	std::uint32_t const action = (function >> action_shift) & action_mask;
	return match != 0 && action >= action_data_value_packet; // 0b10 data value, 0b11 data address
#else
	// 0b0001 to 0b0011: the PC, a data value or both; 0b1100 to 0b1111: data
	// addresses and values. 0b0100 to 0b1011 are watchpoints and triggers.
	return (match >= 0b0001 && match <= 0b0011) || match >= 0b1100;
#endif
}

/**
 * @brief The least capable free comparator that sends data values, or -1:
 *        fewest capabilities, then the highest number. @p ctrl is DWT_CTRL,
 *        @p function_of(n) DWT_FUNCTIONn.
 */
template<typename FunctionOf>
constexpr int least_capable_free(std::uint32_t ctrl, FunctionOf function_of)
{
	if ((ctrl & dwt_notrcpkt) != 0)
		return -1;
	unsigned count = ctrl >> dwt_numcomp_shift;
#if KYMO_DWT_ARMV8M
	count = count < with_value_comparators ? count : with_value_comparators;
#endif
	int chosen = -1;
	int fewest = 0;
	for (unsigned n = 0; n < count; ++n)
	{
		std::uint32_t const function = function_of(n);
		if ((function & match_mask) != 0)
			continue;
#if KYMO_DWT_ARMV8M
		std::uint32_t const id = function >> id_shift;
		if ((id & id_with_value) == 0)
			continue;
		int const capabilities = ones(id);
#else
		int const capabilities = 0;
#endif
		if (chosen < 0 || capabilities <= fewest)
		{
			chosen = static_cast<int>(n);
			fewest = capabilities;
		}
	}
	return chosen;
}

/**
 * @brief Sends each write to the @p size bytes at @p address on SWO, through
 *        the least capable free comparator; false when none is left.
 */
inline bool watch(std::uintptr_t address, std::size_t size)
{
	int const n = least_capable_free(reg(dwt_ctrl), [](unsigned index) { return reg(comparator(index) + function_offset); });
	if (n < 0)
		return false;
	std::uintptr_t const base = comparator(static_cast<unsigned>(n));
	reg(base) = static_cast<std::uint32_t>(address);
#if KYMO_DWT_ARMV8M
	std::uint32_t const datavsize = log2(size) << datavsize_shift;
	reg(base + function_offset) = datavsize | action_data_value | match_write_with_value;
#else
	reg(base + mask_offset) = log2(size);
	reg(base + function_offset) = function_write_value;
#endif
	return true;
}

} // namespace detail

/**
 * @brief Sends the ITM, the DWT's data values with local timestamps, out of
 *        the TPIU as SWO, and puts the anchor on the timestamp word of
 *        @p ring's slot 0. False, with the ITM left off, when no comparator
 *        can send data values.
 *
 * The program configures the SWO pin and the trace clock before.
 */
template<std::size_t Slots>
bool start(ring<Slots> const& ring, swo config)
{
	using namespace detail;
	reg(demcr) |= demcr_trcena;
	reg(itm_lar) = itm_unlock;
	reg(itm_tcr) = 0;
	while ((reg(itm_tcr) & itm_busy) != 0)
	{
	}
	reg(tpiu_cspsr) = 1; // a one-bit port
	reg(tpiu_acpr) = (config.trace_clock / config.baud) - 1;
	reg(tpiu_sppr) = tpiu_nrz;
	reg(tpiu_ffcr) = tpiu_formatter_off;
	reg(dwt_ctrl) |= dwt_cyccntena;
	// A comparator keeps its function across a system reset: the trace of the
	// program that ran before, freed.
	for (unsigned n = 0; n < (reg(dwt_ctrl) >> dwt_numcomp_shift); ++n)
		if (sends_trace(reg(comparator(n) + function_offset)))
			reg(comparator(n) + function_offset) = 0;
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast): the address a comparator matches
	if (!detail::watch(reinterpret_cast<std::uintptr_t>(&ring.slots[0].words[0]), sizeof(std::uint32_t)))
		return false;
	reg(itm_tcr) = itm_itmena | itm_tsena | itm_txena;
	swo_baud.store(config.baud, std::memory_order_relaxed);
	return true;
}

/**
 * @brief Sends every exception's entry, exit and return on SWO, each with a
 *        local timestamp, after start().
 *
 * The DWT traces all exceptions or none: SWO's load follows the program's
 * interrupts. A packet is about 6 bytes with its timestamp; at an 8 MHz core,
 * 2 MBd overflows at 1 kHz of interrupts and 8 MBd keeps up.
 */
inline void exceptions()
{
	detail::reg(detail::dwt_ctrl) |= detail::dwt_exctrcena;
}

/**
 * @brief Sends a tick on SWO every @p Cycles cycles, each with a local
 *        timestamp, after start(): a reference that needs no ring.
 *
 * The local timestamps between two ticks add up to @p Cycles, which lets the
 * host date and check SWO with neither the anchor nor the debug port.
 * @p Cycles is 64 or 1024 times 1 to 16: at most 16384, 2 ms at 8 MHz. A tick
 * takes about 4 bytes with its timestamp: 2 KB/s at 8 MHz and 16384.
 */
template<std::uint32_t Cycles>
void ticks()
{
	using namespace detail;
	constexpr std::uint32_t fields = tick_fields(Cycles);
	static_assert(fields != 0, "a tick period is 64 or 1024 cycles times 1 to 16");
	// The counter's fields are written with the tick off: POSTINIT takes only then.
	std::uint32_t const ctrl = reg(dwt_ctrl) & ~dwt_tick_fields;
	reg(dwt_ctrl) = ctrl;
	reg(dwt_ctrl) = ctrl | (fields & ~dwt_cycevtena);
	reg(dwt_ctrl) = ctrl | fields;
}

/**
 * @brief Sends each write to @p variable on SWO, its value and a local
 *        timestamp; false when no comparator is left. A global of 1, 2 or 4
 *        bytes, aligned on its size.
 */
template<typename Variable>
bool watch(Variable const& variable)
{
	static_assert(std::is_trivially_copyable_v<Variable>, "the host reads the value's bytes");
	static_assert(sizeof(Variable) == 1 || sizeof(Variable) == 2 || sizeof(Variable) == 4,
	              "a comparator watches 1, 2 or 4 bytes");
	static_assert(alignof(Variable) >= sizeof(Variable), "a comparator watches an address aligned on its size");
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast): the address a comparator matches
	return detail::watch(reinterpret_cast<std::uintptr_t>(&variable), sizeof(Variable));
}

} // namespace kymo::dwt
