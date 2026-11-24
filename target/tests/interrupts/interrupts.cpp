/**
 * @file    interrupts.cpp
 * @brief   An image whose DWARF names its interrupts with an IRQn_Type, for
 *          the host to read them (interrupts.py).
 *
 * KYMO_TEST_SVDGENERATOR declares SvdGenerator's: an enumeration in the
 * device's namespace, from interrupt 0. KYMO_TEST_CMSIS links cmsis.c, a
 * vendor's C header: a typedef of an anonymous enumeration that starts with
 * the architecture's exceptions, below zero. With both, the image holds two
 * different IRQn_Type.
 */
#include "../qemu/qemu.hpp"

#include "../test_trace.hpp"

#include <cstdint>

#ifdef KYMO_TEST_SVDGENERATOR
namespace DEVICE
{

/** Device interrupts; the enumerators convert to their number. */
// NOLINTNEXTLINE(cppcoreguidelines-use-enum-class,performance-enum-size): SvdGenerator's declaration, as it emits it
enum IRQn_Type : std::uint16_t
{
	WWDG_IRQn = 0,
	TIM2_IRQn = 45,
};

} // namespace DEVICE

struct svdgenerator_interrupt
{
	DEVICE::IRQn_Type irq;
};
#endif

#ifdef KYMO_TEST_CMSIS
/// The interrupt cmsis.c declares, of its IRQn_Type.
extern "C" std::int32_t cmsis_interrupt();
#endif

struct cmsis_interrupt_seen
{
	std::int32_t irq;
};

void test_main()
{
#ifdef KYMO_TEST_SVDGENERATOR
	trace::emit(svdgenerator_interrupt{DEVICE::TIM2_IRQn});
#endif
#ifdef KYMO_TEST_CMSIS
	trace::emit(cmsis_interrupt_seen{cmsis_interrupt()});
#else
	trace::emit(cmsis_interrupt_seen{0});
#endif
	qemu::finish(true);
}
