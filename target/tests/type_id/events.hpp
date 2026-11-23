#pragma once

#include <kymo/type_id.hpp>

#include <cstdint>

struct event_a { std::uint32_t value; };
struct event_b { std::uint32_t value; };
struct event_c { std::uint32_t value; };

/// Where each translation unit stores the ids it computes.
// NOLINTNEXTLINE(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): volatile stores, which GCC 14 to 16 drop under -flto when made through std::get (a constexpr function returning a volatile reference)
extern std::uint16_t volatile ids_first[3];    // a, b, a again
// NOLINTNEXTLINE(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays): volatile stores, which GCC 14 to 16 drop under -flto when made through std::get (a constexpr function returning a volatile reference)
extern std::uint16_t volatile ids_second[2];   // b, c

void second();
