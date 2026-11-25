/**
 * @file    opsy_hooks.hpp
 * @brief   OpSy's fault image, traced: every event the adapter emits, and
 *          hook::fault recorded from what the adapter wrote to the ring.
 *
 * Put ahead of OpSy's tests/qemu/faults on the fault image's include path.
 * fault_seen repeats what OpSy's fault hooks declare, for test_faults.cpp to
 * compare with what the application's handler is given. Here it is filled
 * from the kymo::fault read back from the ring: the status and EXC_RETURN as
 * recorded, the frame given only when every recorded word of it, and the two
 * fault addresses, match what the hook was given. An event that differs from
 * what the handler gets, or no event, fails OpSy's checks.
 */
#pragma once

#include <opsy_trace.hpp>

#include <kymo/fault.hpp>
#include <kymo/opsy/hooks.hpp>
#include <kymo/ring.hpp>
#include <kymo/type_id.hpp>

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>

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

/**
 * @brief The kymo::fault written at @p index, in @p out; false when the slots
 *        there do not hold one, head first, each with its index's lap.
 */
inline bool read_fault(std::uint32_t index, kymo::fault& out)
{
	using ring_t = decltype(trace_ring);
	constexpr std::size_t count = kymo::slots_for<kymo::fault>;
	std::uint32_t const type = kymo::type_id<kymo::fault>();
	std::array<unsigned char, count * kymo::tail_payload> bytes{};
	for (std::size_t i = 0; i < count; ++i)
	{
		std::uint32_t const at = index + static_cast<std::uint32_t>(i);
		auto const& slot = trace_ring.slots[at & (ring_slots - 1)];
		if (slot.header.load(std::memory_order_acquire) != ring_t::header(type, at, i == 0))
			return false;
		if (i == 0)
			std::memcpy(bytes.data(), &slot.words[1], kymo::head_payload);
		else
			std::memcpy(&bytes[kymo::head_payload + ((i - 1) * kymo::tail_payload)], slot.words.data(), kymo::tail_payload);
	}
	std::memcpy(&out, bytes.data(), sizeof(out));
	return true;
}

class hooks : public kymo::opsy::hooks<::trace>
{
public:
	/** @brief The adapter's hook, then what it wrote, read back into fault_seen. */
	static void fault(auto const& status, auto const& frame, std::uint32_t exc_return)
	{
		std::uint32_t const index = trace_ring.write_index.load(std::memory_order_relaxed);
		kymo::opsy::hooks<::trace>::fault(status, frame, exc_return);

		kymo::fault event{};
		if (!read_fault(index, event))
			return;
		bool const same_frame = event.r0 == frame.r0 && event.r1 == frame.r1 && event.r2 == frame.r2
			&& event.r3 == frame.r3 && event.r12 == frame.r12 && event.lr == frame.lr && event.pc == frame.pc
			&& event.xpsr == frame.xpsr && event.mmfar == status.mmfar && event.bfar == status.bfar;
		fault_seen::status = {.cfsr = event.cfsr, .hfsr = event.hfsr, .mmfar = event.mmfar, .bfar = event.bfar};
		fault_seen::frame = same_frame ? &frame : nullptr;
		fault_seen::exc_return = event.exc_return;
		fault_seen::count = fault_seen::count + 1;
	}
};

} // namespace opsy
