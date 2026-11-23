/**
 * @file    startup.cpp
 * @brief   Reset path of the bench image: .data and .bss, then bench_main.
 *          The vector table stays in flash; only SysTick is used.
 */

#include <array>
#include <cstddef>
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

} // extern "C"

namespace
{

/// The exceptions of an Armv7-M or Armv8-M core, after the reset value of the stack pointer.
constexpr std::size_t exceptions = 15;

/// The vector table: the initial stack pointer, then the exception handlers.
struct vector_table
{
	std::uint32_t const* stack;
	std::array<void (*)(), exceptions> handlers;
};

} // namespace

__attribute__((section(".isr_vector"), used))
constexpr vector_table vectors{
	&_estack,
	{
		&Reset_Handler,
		&Default_Handler,   // NMI
		&Default_Handler,   // HardFault
		&Default_Handler,   // MemManage
		&Default_Handler,   // BusFault
		&Default_Handler,   // UsageFault
		&Default_Handler,   // SecureFault (v8-M)
		nullptr,
		nullptr,
		nullptr,
		&Default_Handler,   // SVCall
		&Default_Handler,   // DebugMonitor
		nullptr,
		&Default_Handler,   // PendSV
		&SysTick_Handler,
	},
};

extern "C" void Reset_Handler()
{
	// The linker symbols bound the sections; to the compiler each is a single
	// word, and it knows nothing of what lies past it.
	// NOLINTBEGIN(cppcoreguidelines-pro-bounds-pointer-arithmetic,clang-analyzer-security.ArrayBound)
	std::uint32_t const* source = &_sidata;
	for (std::uint32_t* destination = &_sdata; destination < &_edata; ++destination, ++source)
		*destination = *source;
	for (std::uint32_t* destination = &_sbss; destination < &_ebss; ++destination)
		*destination = 0;
	// NOLINTEND(cppcoreguidelines-pro-bounds-pointer-arithmetic,clang-analyzer-security.ArrayBound)
	bench_main();
}
