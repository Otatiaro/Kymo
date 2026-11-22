cmake_minimum_required(VERSION 3.25)

# Target-side tests that need only the toolchain. Configure with it, then run
# them with ctest:
#   cmake -S target/tests -B build/tests -G Ninja \
#         -DCMAKE_TOOLCHAIN_FILE=$PWD/target/cmake/arm-none-eabi.cmake
#   cmake --build build/tests && ctest --test-dir build/tests
project(kymo_tests CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

enable_testing()

get_filename_component(bin "${CMAKE_CXX_COMPILER}" DIRECTORY)
find_program(KYMO_READELF arm-none-eabi-readelf HINTS "${bin}" REQUIRED)
find_program(KYMO_NM arm-none-eabi-nm HINTS "${bin}" REQUIRED)

# Type ids, at each optimisation level and with link-time optimisation: two
# translation units sharing a type, linked with --gc-sections and no linker
# script, where any linker warning fails the build.
foreach(cpu cortex-m33 cortex-m4)
	foreach(variant O0 Os O2 Os-lto)
		set(target type-id-${cpu}-${variant})
		string(REPLACE "-lto" "" level ${variant})
		set(options -mcpu=${cpu} -mthumb -${level})
		if(variant MATCHES "lto")
			list(APPEND options -flto)
		endif()
		add_executable(${target} type_id/first.cpp type_id/second.cpp)
		set_target_properties(${target} PROPERTIES SUFFIX ".elf")
		target_include_directories(${target} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../include)
		target_compile_options(${target} PRIVATE ${options}
			-g -Wall -Wextra -Werror
			-ffunction-sections -fdata-sections
			-fno-exceptions -fno-rtti -fno-threadsafe-statics)
		target_link_options(${target} PRIVATE ${options}
			-nostdlib -Wl,--gc-sections -Wl,--fatal-warnings -Wl,-e,test_entry)
		add_test(NAME ${target}
			COMMAND ${CMAKE_COMMAND} -DELF=$<TARGET_FILE:${target}>
				-DREADELF=${KYMO_READELF} -DNM=${KYMO_NM}
				-P ${CMAKE_CURRENT_SOURCE_DIR}/type_id/check.cmake)
	endforeach()
endforeach()
