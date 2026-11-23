/**
 * @file    preempt.cpp
 * @brief   SysTick as an interrupt at a chosen instruction; see preempt.hpp.
 */
#include "preempt.hpp"

#include "qemu.hpp"

#include <cstdint>

namespace
{

std::uint32_t volatile& reg(std::uintptr_t address)
{
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): a register at a fixed address
	return *reinterpret_cast<std::uint32_t volatile*>(address);
}

constexpr std::uintptr_t syst_csr = 0xE000E010U;
constexpr std::uintptr_t syst_rvr = 0xE000E014U;
constexpr std::uintptr_t syst_cvr = 0xE000E018U;
constexpr std::uintptr_t icsr = 0xE000ED04U;
constexpr std::uint32_t csr_enable_interrupt_processor_clock = 7U;
constexpr std::uint32_t icsr_pendstclr = 1U << 25;

constexpr std::uint32_t sled_length = 64;   // NOPs, then a return
constexpr std::uint32_t max_delay = 64 * (sled_length + 1);

bool volatile calibrating;
bool volatile fired;
std::uint32_t volatile interrupted_pc;

} // namespace

extern "C"
{
void sled(std::uint32_t /*unused*/);
/// What the processor stacks on exception entry.
struct exception_frame
{
	std::uint32_t r0, r1, r2, r3, r12, lr, pc, xpsr;
};

void preempt_systick(exception_frame const* frame);
void SysTick_Handler();
}

/// NOPs only, so that instruction n sits at 2 n bytes from the entry.
extern "C" [[gnu::naked, gnu::noinline]] void sled(std::uint32_t /*unused*/)
{
	asm(".rept 64\n\tnop\n\t.endr\n\tbx lr");
}

/// Passes the exception frame on, which holds the interrupted PC.
extern "C" [[gnu::naked]] void SysTick_Handler()
{
	asm("mrs r0, msp\n\tb preempt_systick");
}

extern "C" void preempt_systick(exception_frame const* frame)
{
	// A short delay wraps the counter again during exception entry: the
	// interrupt pending from that wrap is cleared, so the handler runs once.
	reg(syst_csr) = 0;
	reg(icsr) = icsr_pendstclr;
	fired = true;
	interrupted_pc = frame->pc;
	if (!calibrating)
		test_interrupt();
}

namespace preempt
{

void arm(std::uint32_t delay)
{
	reg(syst_rvr) = delay;
	reg(syst_cvr) = 0;
	reg(syst_csr) = csr_enable_interrupt_processor_clock;
}

std::uint32_t interrupted()
{
	return interrupted_pc;
}

[[gnu::noinline]] bool call(std::uint32_t delay, void (*function)(std::uint32_t), std::uint32_t argument)
{
	fired = false;
	arm(delay);
	function(argument);
	reg(syst_csr) = 0;
	reg(icsr) = icsr_pendstclr;
	return fired;
}

void calibrate()
{
	calibrating = true;
	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast): the address of the code, without the Thumb bit
	std::uint32_t const entry = reinterpret_cast<std::uintptr_t>(&sled) & ~1U;
	std::uint32_t next = 0;   // the instruction the next delay must reach
	for (std::uint32_t delay = 1; next <= sled_length; ++delay)
	{
		if (delay > max_delay)
		{
			qemu::write("calibration: the sled outlasted every delay\n");
			qemu::finish(false);
		}
		if (!call(delay, sled, 0))
			break;
		std::uint32_t const offset = interrupted_pc - entry;
		if (offset > 2 * sled_length)
			continue;   // before the sled: the call itself
		std::uint32_t const instruction = offset / 2;
		if (instruction == next)
		{
			++next;
		}
		else if (instruction + 1 != next)
		{
			qemu::write("calibration: delay ");
			qemu::write_unsigned(delay);
			qemu::write(" interrupts instruction ");
			qemu::write_unsigned(instruction);
			qemu::write(" of the sled, expected ");
			qemu::write_unsigned(next);
			qemu::write(" (is -icount shift too small?)\n");
			qemu::finish(false);
		}
	}
	if (next != sled_length + 1)
	{
		qemu::write("calibration: the sled ran out of interrupts\n");
		qemu::finish(false);
	}
	calibrating = false;
}

} // namespace preempt
