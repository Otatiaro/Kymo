#include "events.hpp"

void second()
{
	ids_second[0] = kymo::type_id<event_b>();   // also used in first.cpp
	ids_second[1] = kymo::type_id<event_c>();
}
