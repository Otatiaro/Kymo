/**
 * @file    startup.cpp
 * @brief   Reset path and vector table of the QEMU test images.
 */
#include "qemu.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>

extern "C"
{
extern std::uint32_t _sidata, _sdata, _edata, _sbss, _ebss, _estack;
void Reset_Handler();
void Fault_Handler();
void SysTick_Handler();
}

/// The compiler calls memcpy, memmove, memset and memcmp whenever it likes, and a freestanding
/// program has to provide them; these images link no C library. Built without
/// loop pattern recognition (GCC) or builtins (Clang), so they do not turn into
/// calls to themselves; kept whether referenced or not, since with -flto the
/// calls appear only after the link has dropped what nothing referenced.
#if __has_cpp_attribute(gnu::optimize)
// NOLINTNEXTLINE(cppcoreguidelines-macro-usage): an attribute, which no constant can hold
#define NO_LOOP_PATTERNS gnu::optimize("no-tree-loop-distribute-patterns")
#else
// NOLINTNEXTLINE(cppcoreguidelines-macro-usage): an attribute, which no constant can hold
#define NO_LOOP_PATTERNS clang::no_builtin
#endif

extern "C" [[gnu::used, NO_LOOP_PATTERNS]]
void* memcpy(void* destination, void const* source, std::size_t size)
{
	// NOLINTNEXTLINE(misc-const-correctness): written through *out++, which clang-tidy 23 takes for a read
	auto* out = static_cast<unsigned char*>(destination);
	auto const* from = static_cast<unsigned char const*>(source);
	while (size-- != 0)
		*out++ = *from++;   // NOLINT(cppcoreguidelines-pro-bounds-pointer-arithmetic): memcpy itself
	return destination;
}

extern "C" [[gnu::used, NO_LOOP_PATTERNS]]
void* memmove(void* destination, void const* source, std::size_t size)
{
	// NOLINTNEXTLINE(misc-const-correctness): written through *out, which clang-tidy 23 takes for a read
	auto* out = static_cast<unsigned char*>(destination);
	auto const* from = static_cast<unsigned char const*>(source);
	if (out < from)
		return memcpy(destination, source, size);
	while (size-- != 0)
		out[size] = from[size];   // NOLINT(cppcoreguidelines-pro-bounds-pointer-arithmetic): memmove itself
	return destination;
}

extern "C" [[gnu::used, NO_LOOP_PATTERNS]]
void* memset(void* destination, int value, std::size_t size)
{
	// NOLINTNEXTLINE(misc-const-correctness): written through *out++, which clang-tidy 23 takes for a read
	auto* out = static_cast<unsigned char*>(destination);
	while (size-- != 0)
		*out++ = static_cast<unsigned char>(value);   // NOLINT(cppcoreguidelines-pro-bounds-pointer-arithmetic): memset itself
	return destination;
}

extern "C" [[gnu::used, NO_LOOP_PATTERNS]]
int memcmp(void const* left, void const* right, std::size_t size)
{
	auto const* one = static_cast<unsigned char const*>(left);
	auto const* other = static_cast<unsigned char const*>(right);
	for (; size != 0; --size, ++one, ++other)   // NOLINT(cppcoreguidelines-pro-bounds-pointer-arithmetic): memcmp itself
		if (*one != *other)
			return *one < *other ? -1 : 1;
	return 0;
}

/// Clang calls the run-time ABI's own forms (__aeabi_memcpy, __aeabi_memclr...);
/// the aligned forms (4, 8) are the plain ones. Without builtins under Clang,
/// the calls they make stay calls.
// NOLINTBEGIN(bugprone-reserved-identifier,cert-dcl37-c,cert-dcl51-cpp,readability-identifier-naming): names the ABI fixes
extern "C" [[gnu::used, NO_LOOP_PATTERNS]]
void __aeabi_memcpy(void* destination, void const* source, std::size_t size) noexcept
{
	memcpy(destination, source, size);
}
extern "C" [[gnu::used, gnu::alias("__aeabi_memcpy")]] void __aeabi_memcpy4(void*, void const*, std::size_t) noexcept;
extern "C" [[gnu::used, gnu::alias("__aeabi_memcpy")]] void __aeabi_memcpy8(void*, void const*, std::size_t) noexcept;

extern "C" [[gnu::used, NO_LOOP_PATTERNS]]
void __aeabi_memmove(void* destination, void const* source, std::size_t size) noexcept
{
	memmove(destination, source, size);
}
extern "C" [[gnu::used, gnu::alias("__aeabi_memmove")]] void __aeabi_memmove4(void*, void const*, std::size_t) noexcept;
extern "C" [[gnu::used, gnu::alias("__aeabi_memmove")]] void __aeabi_memmove8(void*, void const*, std::size_t) noexcept;

/// The ABI's memset takes the size before the value.
extern "C" [[gnu::used, NO_LOOP_PATTERNS]]
void __aeabi_memset(void* destination, std::size_t size, int value) noexcept
{
	memset(destination, value, size);
}
extern "C" [[gnu::used, gnu::alias("__aeabi_memset")]] void __aeabi_memset4(void*, std::size_t, int) noexcept;
extern "C" [[gnu::used, gnu::alias("__aeabi_memset")]] void __aeabi_memset8(void*, std::size_t, int) noexcept;

extern "C" [[gnu::used, NO_LOOP_PATTERNS]]
void __aeabi_memclr(void* destination, std::size_t size) noexcept
{
	memset(destination, 0, size);
}
extern "C" [[gnu::used, gnu::alias("__aeabi_memclr")]] void __aeabi_memclr4(void*, std::size_t) noexcept;
extern "C" [[gnu::used, gnu::alias("__aeabi_memclr")]] void __aeabi_memclr8(void*, std::size_t) noexcept;
// NOLINTEND(bugprone-reserved-identifier,cert-dcl37-c,cert-dcl51-cpp,readability-identifier-naming)

extern "C" void Fault_Handler()
{
	qemu::write("unexpected exception\n");
	qemu::finish(false);
}

/// An image that needs SysTick defines it (preempt.cpp does).
extern "C" [[gnu::weak, gnu::alias("Fault_Handler")]] void SysTick_Handler();

/// The vector table: the initial stack pointer, then the exception handlers.
struct vector_table
{
	std::uint32_t const* stack;
	std::array<void (*)(), 15> handlers;
};

__attribute__((section(".isr_vector"), used))
constexpr vector_table vectors{
	&_estack,
	{
		&Reset_Handler,
		&Fault_Handler,   // NMI
		&Fault_Handler,   // HardFault
		&Fault_Handler,   // MemManage
		&Fault_Handler,   // BusFault
		&Fault_Handler,   // UsageFault
		&Fault_Handler,   // SecureFault
		nullptr,
		nullptr,
		nullptr,
		&Fault_Handler,   // SVCall
		&Fault_Handler,   // DebugMonitor
		nullptr,
		&Fault_Handler,   // PendSV
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

	// NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): VTOR, a register at a fixed address
	*reinterpret_cast<std::uint32_t volatile*>(0xE000ED08U) = reinterpret_cast<std::uintptr_t>(&vectors);

	test_main();
	qemu::write("test_main returned\n");
	qemu::finish(false);
}
