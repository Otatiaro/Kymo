/**
 * @file    FreeRTOSConfig.h
 * @brief   The test application's kernel configuration, on the Cortex-M4
 *          (port ARM_CM4F) and the Cortex-M33 (port ARM_CM33_NTZ, the core
 *          left in the Secure state it starts in), under QEMU or, with
 *          KYMO_BOARD, on a NUCLEO-H563ZI.
 */
#ifndef FREERTOS_CONFIG_H
#define FREERTOS_CONFIG_H

#ifdef __cplusplus
extern "C" {
#endif
/** @brief A failed configASSERT: says where, and fails the test. */
void application_assert(char const* file, int line);
#ifdef __cplusplus
}
#endif

#ifdef KYMO_BOARD
#define configCPU_CLOCK_HZ 32000000UL   /* the STM32H563's reset clock: HSI, 64 MHz divided by 2 */
#else
#define configCPU_CLOCK_HZ 25000000UL
#endif
#define configTICK_RATE_HZ 1000
#define configUSE_PREEMPTION 1
#define configUSE_TIME_SLICING 0
#define configMAX_PRIORITIES 6
#define configMINIMAL_STACK_SIZE 256
#define configMAX_TASK_NAME_LEN 12
#define configTICK_TYPE_WIDTH_IN_BITS TICK_TYPE_WIDTH_32_BITS
#define configUSE_IDLE_HOOK 0
#define configUSE_TICK_HOOK 0
#define configUSE_TICKLESS_IDLE 0

#define configSUPPORT_STATIC_ALLOCATION 1
#define configSUPPORT_DYNAMIC_ALLOCATION 0
#define configKERNEL_PROVIDED_STATIC_MEMORY 1

#define configUSE_MUTEXES 1
#define configUSE_RECURSIVE_MUTEXES 1
#define configUSE_COUNTING_SEMAPHORES 1
#define configUSE_TASK_NOTIFICATIONS 1
#define configUSE_TIMERS 0
#define configUSE_TRACE_FACILITY 0

#define INCLUDE_vTaskPrioritySet 1
#define INCLUDE_uxTaskPriorityGet 1
#define INCLUDE_vTaskSuspend 1
#define INCLUDE_vTaskDelete 1
#define INCLUDE_vTaskDelay 1
#define INCLUDE_xTaskDelayUntil 1

/* The kernel at the lowest priority; the handlers that call it at 0x20 or
 * below, in the three upper bits every Cortex-M implements. */
#define configKERNEL_INTERRUPT_PRIORITY 0xE0
#define configMAX_SYSCALL_INTERRUPT_PRIORITY 0x20

/* ARM_CM33_NTZ: no TrustZone split, no MPU, no FPU (the M33 image is built
 * soft-float). */
#define configENABLE_TRUSTZONE 0
#define configRUN_FREERTOS_SECURE_ONLY 1
#define configENABLE_MPU 0
#define configENABLE_FPU 0
#define configENABLE_MVE 0

#define configASSERT(condition)                          \
	do                                                   \
	{                                                    \
		if (!(condition))                                \
			application_assert(__FILE__, __LINE__);      \
	} while (0)

/* The bench's untraced build, the reference its traced build is measured
 * against, defines KYMO_UNTRACED. */
#ifndef KYMO_UNTRACED
#include <kymo/freertos/trace.h>
#endif

#endif
