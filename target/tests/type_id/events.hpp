#pragma once

#include <kymo/type_id.hpp>

#include <cstdint>

struct event_a { std::uint32_t value; };
struct event_b { std::uint32_t value; };
struct event_c { std::uint32_t value; };

/// Where each translation unit stores the ids it computes.
extern volatile std::uint16_t ids_first[3];   // a, b, a again
extern volatile std::uint16_t ids_second[2];  // b, c

void second();
