/**
 * @file    preempt.hpp
 * @brief   An interrupt at any chosen instruction, from SysTick under QEMU's
 *          -icount.
 *
 * With -icount, QEMU's virtual time advances by a fixed amount per
 * instruction and its timers expire exactly on time, so a SysTick armed with
 * a delay of d ticks interrupts the same instruction on every run. When one
 * tick is shorter than one instruction, d + 1 interrupts either the same
 * instruction as d or the next one: sweeping d from 1 interrupts code at
 * each of its instructions in turn, none skipped.
 *
 * calibrate() checks that this holds, on a sled of NOPs, and fails the image
 * otherwise; a run whose -icount shift is too small would skip instructions
 * without anything showing it.
 *
 * Thread mode must run on the main stack, as these images do.
 */
#pragma once

#include <cstdint>

namespace preempt
{

/**
 * @brief Calls @p function with SysTick armed to interrupt after @p delay
 *        ticks (>= 1). @return whether it interrupted before the call ended.
 *
 * Calibration and tests go through this same call, so the instructions
 * between arming and the function's entry are the same for both.
 */
bool call(std::uint32_t delay, void (*function)(std::uint32_t), std::uint32_t argument);

/**
 * @brief Checks that consecutive delays interrupt consecutive instructions of
 *        the called function, from its very first one.
 */
void calibrate();

} // namespace preempt

/// Defined by the test image: what the interrupt does, after calibration.
void test_interrupt();
