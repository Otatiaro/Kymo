/**
 * @file    opsy_hooks.hpp
 * @brief   OpSy's fault image, traced: every event the adapter emits, and
 *          hook::fault recorded as OpSy's own fault hooks record it.
 *
 * Put ahead of OpSy's tests/qemu/faults on the fault image's include path.
 * fault_seen repeats what OpSy's fault hooks declare, for test_faults.cpp to
 * compare with what the application's handler is given.
 */
#pragma once

#include <kymo/opsy/hooks.hpp>

#include <cstdint>

namespace opsy
{

struct exception_frame;

struct fault_seen
{
	static inline volatile std::uint32_t count = 0;
	static inline fault_status status{};
	static inline const exception_frame* frame = nullptr;
	static inline std::uint32_t exc_return = 0;
};

class hooks : public kymo::opsy::hooks
{
public:
	static void fault(const fault_status& status, const exception_frame& frame, std::uint32_t exc_return)
	{
		fault_seen::status = status;
		fault_seen::frame = &frame;
		fault_seen::exc_return = exc_return;
		fault_seen::count = fault_seen::count + 1;
	}
};

} // namespace opsy
