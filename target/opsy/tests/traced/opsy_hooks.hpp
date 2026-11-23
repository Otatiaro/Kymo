/**
 * @file    opsy_hooks.hpp
 * @brief   OpSy's suite, traced: OpSy includes this file when its directory
 *          is on the include path, and calls these hooks.
 */
#pragma once

#include <opsy_trace.hpp>

#include <kymo/opsy/hooks.hpp>

namespace opsy
{
/// A class, not an alias: OpSy's task_control_block names it as a friend.
class hooks : public kymo::opsy::hooks<::trace>
{};
} // namespace opsy
