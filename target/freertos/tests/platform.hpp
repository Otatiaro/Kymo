/**
 * @file    platform.hpp
 * @brief   What the FreeRTOS test images set up before the scheduler starts:
 *          the FPU the hard-float image uses, and a vector table in RAM, as
 *          the table the image links holds no device interrupt.
 */
#pragma once

#include <array>
#include <cstdint>

extern "C"
{
extern std::uint32_t _estack;
void Reset_Handler();
void Fault_Handler();
#ifdef __ARM_ARCH_7EM__
void vPortSVCHandler();
void xPortPendSVHandler();
void xPortSysTickHandler();
#else
void SVC_Handler();
void PendSV_Handler();
void SysTick_Handler();
#endif
}

namespace platform
{

/// The device interrupt the images install a handler for.
constexpr unsigned irq = 0;

using vector = std::uintptr_t;

/// Address of a word of the System Control Space.
inline std::uint32_t volatile& scs(std::uintptr_t address)
{
	return *reinterpret_cast<std::uint32_t volatile*>(address); // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): a register
}

template<typename Function>
vector vector_of(Function* function)
{
	return reinterpret_cast<vector>(function); // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast): a vector table entry
}

/// The port's SysTick handler.
inline void port_systick()
{
#ifdef __ARM_ARCH_7EM__
	xPortSysTickHandler();
#else
	SysTick_Handler();
#endif
}

/**
 * @brief Turns the FPU on where the image uses it, and installs the vector
 *        table: the port's handlers, `systick` for SysTick and `device` for
 *        the device interrupt, at the lowest priority, enabled.
 */
inline void prepare(void (*systick)(), void (*device)())
{
#ifdef __ARM_ARCH_7EM__
	scs(0xE000ED88) |= 0xFU << 20;   // CPACR: CP10 and CP11
	asm volatile("dsb\n\tisb" ::: "memory");
#endif
	// 16 system exceptions and the device interrupt, in a table aligned on
	// its size rounded up to a power of two, as VTOR requires.
	alignas(128) static std::array<vector, 32> vectors;
	vectors.fill(vector_of(&Fault_Handler));
	vectors[0] = reinterpret_cast<vector>(&_estack); // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast): the initial stack pointer
	vectors[1] = vector_of(&Reset_Handler);
#ifdef __ARM_ARCH_7EM__
	vectors[11] = vector_of(&vPortSVCHandler);
	vectors[14] = vector_of(&xPortPendSVHandler);
#else
	vectors[11] = vector_of(&SVC_Handler);
	vectors[14] = vector_of(&PendSV_Handler);
#endif
	vectors[15] = vector_of(systick);
	vectors[16 + irq] = vector_of(device);
	scs(0xE000ED08) = reinterpret_cast<std::uintptr_t>(vectors.data()); // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast): VTOR
	asm volatile("dsb\n\tisb" ::: "memory");

	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr,cppcoreguidelines-pro-bounds-pointer-arithmetic): NVIC_IPR, a byte per interrupt
	reinterpret_cast<std::uint8_t volatile*>(0xE000E400)[irq] = 0xFF;   // the lowest, below configMAX_SYSCALL_INTERRUPT_PRIORITY
	scs(0xE000E100) = 1U << irq;   // NVIC_ISER0
}

/// Pends the device interrupt: from a task, its handler runs at once.
inline void pend()
{
	scs(0xE000E200) = 1U << irq;   // NVIC_ISPR0
}

/**
 * @brief Spins for `milliseconds` on SysTick, polled, before the first event:
 *        the time the probe takes to attach once the board is reset, so that
 *        the recording starts with the application. The port sets SysTick up
 *        again when the scheduler starts.
 */
inline void wait(std::uint32_t milliseconds, std::uint32_t core_clock)
{
	scs(0xE000E014) = (core_clock / 1000) - 1;   // SYST_RVR
	scs(0xE000E018) = 0;                       // SYST_CVR
	scs(0xE000E010) = 5;                       // SYST_CSR: the core's clock, enabled
	for (std::uint32_t elapsed = 0; elapsed < milliseconds;)
		if ((scs(0xE000E010) & (1U << 16)) != 0)   // COUNTFLAG, cleared by the read
			++elapsed;
	scs(0xE000E010) = 0;
}

} // namespace platform
