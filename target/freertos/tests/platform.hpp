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

/// A memory-mapped register: of the System Control Space, or of a peripheral.
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
#ifdef __ARM_FP
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

#ifdef KYMO_BENCH_BOARD
/**
 * @brief STM32H563 to 250 MHz: VOS0, flash at 5 wait states, PLL1 from the
 *        32 MHz HSI (/8, x125, /2), instruction cache on. OpSy's bench image
 *        sets the same, so that the kernels are timed on the same memory
 *        system.
 */
inline void clock_250mhz()
{
	constexpr std::uintptr_t pwr_voscr = 0x44020810, pwr_vossr = 0x44020814;
	constexpr std::uintptr_t flash_acr = 0x40022000;
	constexpr std::uintptr_t rcc_cr = 0x44020C00, rcc_cfgr1 = 0x44020C1C, rcc_cfgr2 = 0x44020C20;
	constexpr std::uintptr_t rcc_pll1cfgr = 0x44020C28, rcc_pll1divr = 0x44020C34;
	constexpr std::uintptr_t icache_cr = 0x40030400, icache_sr = 0x40030404;

	scs(pwr_voscr) = 3U << 4;                                 // VOS0
	while ((scs(pwr_vossr) & (1U << 3)) == 0) {}              // VOSRDY
	scs(flash_acr) = 5U | (2U << 4) | (1U << 8);              // LATENCY 5, WRHIGHFREQ 2, PRFTEN
	while ((scs(flash_acr) & 0xFU) != 5U) {}                  // the write lands before the clock rises
	scs(rcc_pll1cfgr) = 0b01U | (0b10U << 2) | (8U << 8) | (1U << 16);   // HSI, 4-8 MHz in, M 8, P out
	scs(rcc_pll1divr) = 124U | (1U << 9);                     // N 125, P 2: 32 / 8 * 125 / 2 = 250 MHz
	scs(rcc_cr) |= 1U << 24;                                  // PLL1ON
	while ((scs(rcc_cr) & (1U << 25)) == 0) {}                // PLL1RDY
	scs(rcc_cfgr2) = 0;                                       // every prescaler at 1
	scs(rcc_cfgr1) = 0b011U;                                  // SW: PLL1
	while (((scs(rcc_cfgr1) >> 3) & 0b11U) != 0b011U) {}      // SWS
	while ((scs(icache_sr) & 1U) != 0) {}                     // BUSYF
	scs(icache_cr) |= 1U;                                     // EN
}

/// Starts the DWT cycle counter.
inline void start_cycle_count()
{
	scs(0xE000EDFC) |= 1U << 24;   // DEMCR: TRCENA
	scs(0xE0001004) = 0;           // DWT_CYCCNT
	scs(0xE0001000) |= 1U;         // DWT_CTRL: CYCCNTENA
}

/// The core cycles since the counter started, modulo 2^32.
inline std::uint32_t cycle_count()
{
	return scs(0xE0001004);   // DWT_CYCCNT
}
#endif

} // namespace platform
