/**
 * @file    qemu.hpp
 * @brief   What a test image needs under QEMU: semihosting output and exit,
 *          and the entry point that startup.cpp calls.
 */
#pragma once

#include <cstdint>

namespace qemu
{

inline int semihosting_call(int operation, void const* argument)
{
	register int r0 asm("r0") = operation;
	register void const* r1 asm("r1") = argument;
	asm volatile("bkpt 0xAB" : "+r"(r0) : "r"(r1) : "memory");
	return r0;
}

/** @brief SYS_WRITE0: a NUL-terminated string to QEMU's stdout. */
inline void write(char const* text)
{
	semihosting_call(0x04, text);
}

inline void write_unsigned(std::uint32_t value)
{
	char digits[11];
	int index = 10;
	digits[index] = '\0';
	do
	{
		digits[--index] = static_cast<char>('0' + value % 10);
		value /= 10;
	} while (value != 0);
	write(&digits[index]);
}

/** @brief SYS_EXIT_EXTENDED: QEMU exits with status 0 for 0, 1 otherwise. */
[[noreturn]] inline void exit(int code)
{
	std::uint32_t const parameters[2] = {0x20026u, static_cast<std::uint32_t>(code)};   // ADP_Stopped_ApplicationExit
	semihosting_call(0x20, parameters);
	for (;;)
	{
	}
}

/** @brief Prints the verdict line the runner looks for, and exits. */
[[noreturn]] inline void finish(bool pass)
{
	write(pass ? "RESULT: PASS\n" : "RESULT: FAIL\n");
	exit(pass ? 0 : 1);
}

} // namespace qemu

/// Defined by the test image.
void test_main();
