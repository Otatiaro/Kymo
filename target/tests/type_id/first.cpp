#include "events.hpp"

volatile std::uint16_t ids_first[3];
volatile std::uint16_t ids_second[2];

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
