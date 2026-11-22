/**
 * @file    startup.cpp
 * @brief   Reset path and vector table of the QEMU test images.
 */
#include "qemu.hpp"

#include <cstddef>
#include <cstdint>

extern "C"
{
extern std::uint32_t _sidata, _sdata, _edata, _sbss, _ebss, _estack;
void Reset_Handler();
void Fault_Handler();
void SysTick_Handler();
}

/// GCC calls memcpy and memset whenever it likes, and a freestanding program
/// has to provide them; these images link no C library. Built without loop
/// pattern recognition, so they do not turn into calls to themselves.
extern "C" [[gnu::optimize("no-tree-loop-distribute-patterns")]]
void* memcpy(void* destination, void const* source, std::size_t size)
{
	auto* to = static_cast<unsigned char*>(destination);
	auto const* from = static_cast<unsigned char const*>(source);
	while (size-- != 0)
		*to++ = *from++;
	return destination;
}

extern "C" [[gnu::optimize("no-tree-loop-distribute-patterns")]]
void* memset(void* destination, int value, std::size_t size)
{
	auto* to = static_cast<unsigned char*>(destination);
	while (size-- != 0)
		*to++ = static_cast<unsigned char>(value);
	return destination;
}

extern "C" void Fault_Handler()
{
	qemu::write("unexpected exception\n");
	qemu::finish(false);
}

/// An image that needs SysTick defines it (preempt.cpp does).
extern "C" [[gnu::weak, gnu::alias("Fault_Handler")]] void SysTick_Handler();

__attribute__((section(".isr_vector"), used))
void const* const vectors[16] = {
	&_estack,
	reinterpret_cast<void const*>(&Reset_Handler),
	reinterpret_cast<void const*>(&Fault_Handler),   // NMI
	reinterpret_cast<void const*>(&Fault_Handler),   // HardFault
	reinterpret_cast<void const*>(&Fault_Handler),   // MemManage
	reinterpret_cast<void const*>(&Fault_Handler),   // BusFault
	reinterpret_cast<void const*>(&Fault_Handler),   // UsageFault
	reinterpret_cast<void const*>(&Fault_Handler),   // SecureFault
	nullptr, nullptr, nullptr,
	reinterpret_cast<void const*>(&Fault_Handler),   // SVCall
	reinterpret_cast<void const*>(&Fault_Handler),   // DebugMonitor
	nullptr,
	reinterpret_cast<void const*>(&Fault_Handler),   // PendSV
	reinterpret_cast<void const*>(&SysTick_Handler),
};

extern "C" void Reset_Handler()
{
	std::uint32_t const* source = &_sidata;
	for (std::uint32_t* destination = &_sdata; destination < &_edata; ++destination, ++source)
		*destination = *source;
	for (std::uint32_t* destination = &_sbss; destination < &_ebss; ++destination)
		*destination = 0;

	*reinterpret_cast<std::uint32_t volatile*>(0xE000ED08u) = reinterpret_cast<std::uint32_t>(&vectors[0]);   // VTOR

	test_main();
	qemu::write("test_main returned\n");
	qemu::finish(false);
}
