/**
 * @file    record.cpp
 * @brief   Events written without end, for the host to record through QEMU's
 *          monitor while the CPU runs (record.py).
 *
 * Events of one, two and four slots, each carrying its own ring index and
 * values worked out from it: what the host prints shows on its own whether
 * it read the event that was written. Bursts that overtake the host
 * alternate with pauses it can catch up in.
 */
#include "events.hpp"
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

#include <atomic>
#include <cstdint>

namespace
{

/// Events in each burst, and loop rounds in each pause.
constexpr std::uint32_t burst = 64;
constexpr std::uint32_t pause = 200000;

} // namespace

void test_main()
{
	for (std::uint32_t n = 0;; ++n)
	{
		emit_numbered(n, kymo::trace_ring.write_index.load(std::memory_order_relaxed));
		if (n / burst % 2 == 1)
			for (std::uint32_t volatile k = 0; k < pause; k = k + 1)
			{
			}
	}
}
