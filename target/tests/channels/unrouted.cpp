/**
 * @file    unrouted.cpp
 * @brief   A tracer that routes the application family alone, emitting a
 *          scheduling event: it must not compile. With KYMO_TEST_ROUTED, the
 *          scheduling family has its route, and it compiles.
 */
#include <kymo/kymo.hpp>

#include <cstdint>

inline kymo::ring<16> app_ring;
inline kymo::ring<16> os_ring;

struct trace : kymo::tracer<trace>
{
	static auto& channel(kymo::on<kymo::family::application> /*family*/)
	{
		return app_ring;
	}

#ifdef KYMO_TEST_ROUTED
	static auto& channel(kymo::on<kymo::family::scheduling> /*family*/)
	{
		return os_ring;
	}
#endif
};

struct switched
{
	static constexpr kymo::family kymo_family = kymo::family::scheduling;
	std::uint32_t n;
};

void emit_switched(std::uint32_t n);

void emit_switched(std::uint32_t n)
{
	trace::emit(switched{n});
}
