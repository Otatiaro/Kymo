#include "events.hpp"

#include <kymo/type_id.hpp>

#include <cstdint>

// NOLINTNEXTLINE(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): volatile stores, which GCC 14 to 16 drop under -flto when made through std::get (a constexpr function returning a volatile reference)
std::uint16_t volatile ids_first[3];
// NOLINTNEXTLINE(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): volatile stores, which GCC 14 to 16 drop under -flto when made through std::get (a constexpr function returning a volatile reference)
std::uint16_t volatile ids_second[2];

extern "C" [[noreturn]] void test_entry()
{
	ids_first[0] = kymo::type_id<event_a>();
	ids_first[1] = kymo::type_id<event_b>();
	ids_first[2] = kymo::type_id<event_a>();   // inlined twice in one unit
	second();
	for (;;)
	{
	}
}
