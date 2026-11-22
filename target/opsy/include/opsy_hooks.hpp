/**
 * @file    opsy_hooks.hpp
 * @brief   Traces an OpSy program with Kymo: OpSy includes this file when
 *          its directory is on the include path, and calls these hooks.
 *
 * A program with hooks of its own writes its own opsy_hooks.hpp instead:
 * see kymo/opsy/hooks.hpp.
 */
#pragma once

#include <kymo/opsy/hooks.hpp>

namespace opsy
{
/// A class, not an alias: OpSy's task_control_block names it as a friend.
class hooks : public kymo::opsy::hooks
{};
}
