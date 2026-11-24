/**
 * @file    cmsis.c
 * @brief   A vendor's CMSIS device header, as C declares it: the
 *          architecture's exceptions below zero, then the device's
 *          interrupts from zero.
 */
#include <stdint.h>

typedef enum
{
	NonMaskableInt_IRQn = -14,
	HardFault_IRQn = -13,
	SVCall_IRQn = -5,
	PendSV_IRQn = -2,
	SysTick_IRQn = -1,
	WWDG_IRQn = 0,
	TIM2_IRQn = 28,
} IRQn_Type;

/** An interrupt, kept for its type to stay in the DWARF. */
IRQn_Type volatile cmsis_last = TIM2_IRQn;

int32_t cmsis_interrupt(void)
{
	return (int32_t)cmsis_last;
}
