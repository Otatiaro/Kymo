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

#include <cstdint>

namespace kymo
{

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
};

} // namespace kymo
