/**
 * @file    opsy_hooks.hpp
 * @brief   The call levels image's hooks: the adapter's, with where in the
 *          ring the last fault's event starts, for levels.cpp to read it back.
 */
#pragma once

#include <opsy_trace.hpp>

#include <kymo/opsy/hooks.hpp>

#include <atomic>
#include <cstdint>

namespace opsy
{
/// A class, not an alias: OpSy's task_control_block names it as a friend.
class hooks : public kymo::opsy::hooks<::trace>
{
public:
	/// The ring index the last kymo::fault starts at.
	static inline std::uint32_t fault_index = 0;

	static void fault(auto const& status, auto const& frame, std::uint32_t exc_return)
	{
		fault_index = trace_ring.write_index.load(std::memory_order_relaxed);
		kymo::opsy::hooks<::trace>::fault(status, frame, exc_return);
	}
};
} // namespace opsy
