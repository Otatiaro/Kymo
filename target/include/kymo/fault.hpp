/**
 * @file    fault.hpp
 * @brief   The fault event: what a Cortex-M fault leaves to read, as the fault
 *          handler finds it.
 *
 * The status registers as read on entry to the handler, the frame the core
 * stacked on exception entry, and the EXC_RETURN the handler was entered with,
 * which says which stack the frame is on. The task that faulted is not in it:
 * the host knows it from the scheduling events that come before.
 *
 * With them, the frame's address, the bounds of the stack it should be on when
 * the adapter knows them, and up to eight words above the frame that may be
 * return addresses: what the host makes the first call levels from, the
 * program having reset since. When the bounds are known, they are searched
 * only if the frame lies inside them.
 *
 * A frame the core could not stack (CFSR's MSTKERR or STKERR set) is recorded
 * as found; the host says it is not to be trusted. MMFAR and BFAR hold an
 * address only when CFSR's MMARVALID or BFARVALID says so.
 *
 * Emitting it records nothing more: what the program does next, freeze the
 * trace, reset, wait for the host, is its own choice (kymo::freeze() keeps the
 * events before the fault from being overwritten).
 */
#pragma once

#include <kymo/control.hpp>

#include <array>
#include <cstddef>
#include <cstdint>

namespace kymo
{

/** @brief How many words that may be return addresses a fault event carries. */
inline constexpr std::size_t fault_return_count = 8;

/** @brief Those words, nearest the frame first; 0 past the last. */
using fault_return_words = std::array<std::uint32_t, fault_return_count>;

/** @brief A Cortex-M fault (ARMv7-M, ARMv8-M Mainline). The host reads the fields by name. */
struct fault
{
	static constexpr family kymo_family = family::faults;

	std::uint32_t cfsr;  ///< Configurable Fault Status Register: MMFSR, BFSR, UFSR, low byte up
	std::uint32_t hfsr;  ///< HardFault Status Register
	std::uint32_t mmfar; ///< MemManage Fault Address Register
	std::uint32_t bfar;  ///< BusFault Address Register

	// The frame the core stacked, in its order.
	std::uint32_t r0;
	std::uint32_t r1;
	std::uint32_t r2;
	std::uint32_t r3;
	std::uint32_t r12;
	std::uint32_t lr;
	std::uint32_t pc;
	std::uint32_t xpsr;

	std::uint32_t exc_return; ///< the handler's LR on entry

	std::uint32_t sp;         ///< the frame's address
	std::uint32_t stack_base; ///< the lowest address of the stack the frame should be on, 0 when unknown
	std::uint32_t stack_top;  ///< the address that stack starts from, 0 when unknown

	fault_return_words returns; ///< words above the frame that may be return addresses
};

/** @brief How many words above a fault's frame fault_returns() reads, at most. */
inline constexpr std::uint32_t fault_search_words = 64;

/**
 * @brief The words above the frame at @p frame_address that may be return addresses:
 *        odd, as a Thumb return address is, and in the Code region.
 *
 * Reads from the end of the frame, its floating-point part included when
 * EXC_RETURN says one was stacked, up to @p limit or fault_search_words
 * further, whichever comes first. The caller makes sure that much is
 * readable. Without frame pointers nor unwind tables on the target, which
 * word holds which function's LR is unknown here: the host keeps the words
 * that follow a call in the program.
 */
inline fault_return_words fault_returns(std::uint32_t frame_address, std::uint32_t exc_return, std::uint32_t limit) noexcept
{
	constexpr std::uint32_t basic_frame = 8 * 4;     // r0-r3, r12, lr, pc, xPSR
	constexpr std::uint32_t extended_frame = 26 * 4; // and s0-s15, FPSCR, a reserved word
	constexpr std::uint32_t no_fp_frame = 1U << 4;   // EXC_RETURN.FType
	constexpr std::uint32_t code_end = 0x20000000;

	std::uint32_t address = frame_address + ((exc_return & no_fp_frame) != 0 ? basic_frame : extended_frame);
	std::uint32_t const end = (limit > address && limit - address > fault_search_words * 4) ? address + (fault_search_words * 4) : limit;
	fault_return_words found{};
	std::size_t count = 0;
	for (; address < end && count < found.size(); address += 4)
	{
		// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): a word of the stack, by its address
		std::uint32_t const word = *reinterpret_cast<std::uint32_t const volatile*>(address);
		if ((word & 1U) != 0 && word < code_end)
			found[count++] = word;
	}
	return found;
}

/** @brief Where the main stack starts: the initial stack pointer, first word of the vector table VTOR points to. */
inline std::uint32_t main_stack_top() noexcept
{
	constexpr std::uintptr_t vtor = 0xE000ED08;
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): a register at a fixed address, then the word it points to
	return **reinterpret_cast<std::uint32_t const* const volatile*>(vtor);
}

} // namespace kymo
