// What std::format writes for each format specification Kymo admits, and some
// it rejects, applied to values of each class: the reference format.rs is
// tested against (reference.txt, read by tests.rs).
//
// One line per case, tab-separated: the value's type, the specification, the
// value (an integer in decimal, a floating-point number by its bits in
// hexadecimal, a bool as 0 or 1, a char by its code, a string as is), and
// what std::format writes, or `error` when it rejects the specification.
//
//     g++ -std=c++20 -O1 reference.cpp -o reference && ./reference > reference.txt
//     cl /std:c++20 /EHsc /utf-8 reference.cpp && reference > reference.txt

#include <bit>
#include <cstdint>
#include <cstdio>
#include <format>
#include <limits>
#include <string>
#include <string_view>
#include <vector>

namespace
{

template<typename T>
std::string format(std::string_view spec, T value)
{
	try
	{
		return std::vformat("{:" + std::string(spec) + "}", std::make_format_args(value));
	}
	catch (std::format_error const&)
	{
		return "error";
	}
}

template<typename T>
void cases(char const* type, std::vector<std::string_view> const& specs, std::vector<T> const& values)
{
	for (auto spec : specs)
	{
		for (T value : values)
		{
			std::string shown;
			if constexpr (std::is_same_v<T, float>)
				shown = std::format("{:#010x}", std::bit_cast<std::uint32_t>(value));
			else if constexpr (std::is_same_v<T, double>)
				shown = std::format("{:#018x}", std::bit_cast<std::uint64_t>(value));
			else if constexpr (std::is_same_v<T, std::string_view>)
				shown = value;
			else
				shown = std::to_string(value);
			std::printf("%s\t%.*s\t%s\t%s\n", type, static_cast<int>(spec.size()), spec.data(), shown.c_str(),
			            format(spec, value).c_str());
		}
	}
}

template<typename T>
void integers(char const* type, std::vector<std::string_view> const& specs)
{
	using limits = std::numeric_limits<T>;
	std::vector<T> values{T{0}, T{1}, T{42}, static_cast<T>(255 > limits::max() ? 100 : 255), limits::max(),
	                      limits::min()};
	if constexpr (limits::is_signed)
	{
		values.push_back(T{-1});
		values.push_back(T{-42});
	}
	cases(type, specs, values);
}

} // namespace

int main()
{
	std::vector<std::string_view> const integer_specs{
	    "",    "d",    "+",     " ",     "-",    "x",     "X",    "#x",   "#X",     "o",      "#o",  "b",
	    "B",   "#b",   "#B",    "08",    "+08",  "#010x", "-#6X", ">6",   "<6",     "^7",     "*^9", "0>8",
	    "+^9", "+#012b", "#08o", ".2",   "s",    "f",     "e",    "g",    "{}",
	};
	integers<std::int8_t>("i8", integer_specs);
	integers<std::uint8_t>("u8", integer_specs);
	integers<std::int16_t>("i16", integer_specs);
	integers<std::uint16_t>("u16", integer_specs);
	integers<std::int32_t>("i32", integer_specs);
	integers<std::uint32_t>("u32", integer_specs);
	integers<std::int64_t>("i64", integer_specs);
	integers<std::uint64_t>("u64", integer_specs);

	std::vector<std::string_view> const float_specs{
	    "",      ".0",     ".1",    ".3",    ".10",   ".17",   "e",      "E",    ".0e",   ".2e",   ".12e",
	    "f",     ".0f",    ".2f",   ".10f",  "F",     "g",     "G",      ".3g",  ".0g",   ".1g",   ".17g",
	    "#g",    "#.3g",   "#",     "#.0",   "#.0f",  "#.0e",  "#e",     "+",    " ",     "08.2f", "+010.3e",
	    "<10",   ">10",    "^10.2f", "*^12g", "010",  "-012",  "+012g",  "x",    "d",     "s",
	};
	double const inf = std::numeric_limits<double>::infinity();
	double const nan = std::numeric_limits<double>::quiet_NaN();
	cases<double>("f64", float_specs,
	              {0.0,
	               -0.0,
	               1.0,
	               -1.5,
	               0.1,
	               2.5,
	               0.125,
	               0.375,
	               1e-5,
	               1.5e-5,
	               0.0001,
	               0.00001234,
	               100.0,
	               123456.0,
	               1234567.0,
	               9.999999,
	               99.995,
	               3.141592653589793,
	               1e10,
	               1e15,
	               1e16,
	               1e17,
	               1e21,
	               1e22,
	               1e23,
	               123456789012345680000.0,
	               1e-300,
	               5e-324,
	               std::numeric_limits<double>::max(),
	               inf,
	               -inf,
	               nan,
	               -nan});
	float const finf = std::numeric_limits<float>::infinity();
	cases<float>("f32", float_specs,
	             {0.0F, -0.0F, 1.0F, -1.5F, 0.1F, 2.5F, 0.125F, 1e-5F, 123456.0F, 16777216.0F, 3.14159265F, 1e10F,
	              9.999999F, 1e-45F, std::numeric_limits<float>::max(), finf, -finf,
	              std::numeric_limits<float>::quiet_NaN()});

	cases<bool>("bool", {"", "s", "d", "x", "#x", "#b", "08d", "+d", ">6", "<6", "^7", "*>7", "+", "#", "0", ".2", "e", "f"},
	            {false, true});
	cases<char>("char", {"", "c", "d", "#x", "X", "+d", "05d", ">4", "<4", "^5", "+", "#", "0", ".2", "s", "e"},
	            {'A', 'z', '0', '~'});
	cases<std::string_view>("str",
	                        {"", "s", ">8", "<8", "^9", "*^9", ".2", ".2s", "10.3", ".0", "+", "#", "0", "d", "x"},
	                        {"IDLE", "", "hello world", "x"});
}
