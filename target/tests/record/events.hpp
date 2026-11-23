/**
 * @file    events.hpp
 * @brief   The events of the record images, each carrying its own ring index
 *          and values worked out from it, as record.py expects them.
 */
#pragma once

#include <kymo/kymo.hpp>

#include <cstdint>

struct one
{
	std::uint32_t index;
};

struct two
{
	std::uint32_t index;
	std::uint32_t words[3];
};

struct four
{
	std::uint32_t index;
	std::uint32_t words[10];
};

/** @brief The type of the @p n th event: one, two and four in turn. */
inline std::uint32_t type_of(std::uint32_t n)
{
	switch (n % 3)
	{
	case 0:
		return kymo::type_id<one>();
	case 1:
		return kymo::type_id<two>();
	default:
		return kymo::type_id<four>();
	}
}

/**
 * @brief Emits the @p n th event, with the values of @p index: the index the
 *        ring gives it, as long as nothing else emits before it reserves.
 */
inline void emit_numbered(std::uint32_t n, std::uint32_t index)
{
	switch (n % 3)
	{
	case 0:
		kymo::emit(one{index});
		break;
	case 1:
		kymo::emit(two{index, {index + 1, index + 2, index + 3}});
		break;
	default:
	{
		four event{index, {}};
		for (std::uint32_t k = 0; k < 10; ++k)
			event.words[k] = index * 10 + k;
		kymo::emit(event);
		break;
	}
	}
}
