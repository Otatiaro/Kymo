/**
 * @file    startup.cpp
 * @brief   Reset path of the bench image: .data and .bss, then bench_main.
 *          The vector table stays in flash; only SysTick is used.
 */

#include <cstdint>

extern "C"
{

extern std::uint32_t _estack;
extern std::uint32_t _sidata;
extern std::uint32_t _sdata;
extern std::uint32_t _edata;
extern std::uint32_t _sbss;
extern std::uint32_t _ebss;

void Reset_Handler();
void Default_Handler();
void SysTick_Handler();
[[noreturn]] void bench_main();

void Default_Handler()
{
	for (;;)
	{
	}
}

__attribute__((section(".isr_vector"), used))
const void* const vectors[16] = {
	&_estack,
	reinterpret_cast<const void*>(&Reset_Handler),
	reinterpret_cast<const void*>(&Default_Handler),   // NMI
	reinterpret_cast<const void*>(&Default_Handler),   // HardFault
	reinterpret_cast<const void*>(&Default_Handler),   // MemManage
	reinterpret_cast<const void*>(&Default_Handler),   // BusFault
	reinterpret_cast<const void*>(&Default_Handler),   // UsageFault
	reinterpret_cast<const void*>(&Default_Handler),   // SecureFault (v8-M)
	nullptr, nullptr, nullptr,
	reinterpret_cast<const void*>(&Default_Handler),   // SVCall
	reinterpret_cast<const void*>(&Default_Handler),   // DebugMonitor
	nullptr,
	reinterpret_cast<const void*>(&Default_Handler),   // PendSV
	reinterpret_cast<const void*>(&SysTick_Handler),
};

void Reset_Handler()
{
	const std::uint32_t* source = &_sidata;
	for (std::uint32_t* destination = &_sdata; destination < &_edata; ++destination, ++source)
		*destination = *source;
	for (std::uint32_t* destination = &_sbss; destination < &_ebss; ++destination)
		*destination = 0;
	bench_main();
}

} // extern "C"
