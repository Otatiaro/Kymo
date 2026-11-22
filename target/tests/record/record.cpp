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
#include "../qemu/qemu.hpp"

#include <kymo/kymo.hpp>

#include <cstdint>

struct one
{
	std::uint32_t index;
};

struct two
{
	std::uint32_t index;
	std::uint32_t words[3];
};

struct four
{
	std::uint32_t index;
	std::uint32_t words[10];
};

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
		std::uint32_t const index = kymo::trace_ring.write_index.load(std::memory_order_relaxed);
		switch (n % 3)
		{
		case 0:
			kymo::emit(one{index});
			break;
		case 1:
			kymo::emit(two{index, {index + 1, index + 2, index + 3}});
			break;
		default:
		{
			four event{index, {}};
			for (std::uint32_t k = 0; k < 10; ++k)
				event.words[k] = index * 10 + k;
			kymo::emit(event);
			break;
		}
		}
		if (n / burst % 2 == 1)
			for (std::uint32_t volatile k = 0; k < pause; k = k + 1)
			{
			}
	}
}
