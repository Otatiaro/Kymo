/**
 * @file    type_id.hpp
 * @brief   Event type ids, assigned by the linker with no registry and no
 *          linker script.
 *
 * Each event type owns one byte in .kymo.ids, a section that is not loaded
 * on the target: the linker places it at address 0, so the address of the
 * byte is its offset in the section, and that offset is the type's id. The
 * host reads the ids back from the ELF symbol table: the symbol of a type is
 * the mangled name of kymo::anchor<Event>::name followed by ".kymo_id".
 *
 * - One byte per type across translation units: the byte sits in a COMDAT
 *   group named after its symbol, and the linker keeps one group per name.
 * - One byte per type within a translation unit: .ifndef skips the
 *   definition when type_id is inlined more than once.
 * - At most 65 536 types: the id is the low 16 bits of the symbol, loaded
 *   with a single MOVW, so it costs one instruction and no flash.
 *
 * MOVW needs Thumb-2: ARMv7-M, ARMv7E-M, ARMv8-M.
 */
#pragma once

#include <cstdint>

#ifdef __ARM_ARCH_6M__
#error "kymo::type_id needs MOVW, which ARMv6-M (Cortex-M0, M0+) lacks"
#endif

namespace kymo
{

/** @brief Never defined: its mangled name names the type's id symbol. */
template<typename Event>
struct anchor
{
	static char const name;
};

/**
 * @brief The id of @p Event, a link-time constant, zero-extended to a word:
 *        MOVW clears the upper half, which the compiler cannot see through
 *        the asm, so the word form spares a zero-extension.
 */
template<typename Event>
[[gnu::always_inline]] inline std::uint32_t type_id_word()
{
	std::uint32_t id = 0;
	asm(".ifndef %c1.kymo_id\n\t"
	    ".pushsection .kymo.ids,\"G\",%%progbits,%c1.kymo_id,comdat\n\t"
	    ".weak %c1.kymo_id\n"
	    "%c1.kymo_id:\n\t"
	    ".byte 0\n\t"
	    ".popsection\n\t"
	    ".endif\n\t"
	    "movw %0, #:lower16:%c1.kymo_id"
	    : "=r"(id)
	    : "i"(&anchor<Event>::name));
	return id;
}

/** @brief The id of @p Event, a link-time constant. */
template<typename Event>
[[gnu::always_inline]] inline std::uint16_t type_id()
{
	return static_cast<std::uint16_t>(type_id_word<Event>());
}

} // namespace kymo
