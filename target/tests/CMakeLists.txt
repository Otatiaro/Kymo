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

# Images run under QEMU. Without QEMU or Python the tests are still
# registered, and fail saying what is missing.
find_program(KYMO_QEMU qemu-system-arm HINTS "C:/Program Files/qemu")
find_package(Python3 COMPONENTS Interpreter)

set(missing "")
foreach(tool KYMO_QEMU Python3_EXECUTABLE)
	if(NOT ${tool})
		list(APPEND missing ${tool})
	endif()
endforeach()

# Each instruction takes 2^shift ns of virtual time. An instruction must last
# longer than a SysTick tick for an interrupt to land on every instruction
# (qemu/preempt.hpp). The images check it: at shift 6, four of the six already
# miss the first instruction of the called function; 7 is the lowest that holds.
set(KYMO_ICOUNT_SHIFT 7 CACHE STRING "QEMU -icount shift for the preemption tests")

# Lock-free writes: an interrupt that emits, at every instruction of an emit.
foreach(cpu cortex-m33 cortex-m4)
	if(cpu STREQUAL cortex-m33)
		set(machine mps2-an505)
		set(script ${CMAKE_CURRENT_SOURCE_DIR}/qemu/armv8m.ld)
	else()
		set(machine mps2-an386)
		set(script ${CMAKE_CURRENT_SOURCE_DIR}/qemu/armv7m.ld)
	endif()
	foreach(level O0 Os O2)
		set(target emit-preemption-${cpu}-${level})
		add_executable(${target} emit/preemption.cpp qemu/preempt.cpp qemu/startup.cpp)
		set_target_properties(${target} PROPERTIES SUFFIX ".elf" LINK_DEPENDS ${script})
		target_include_directories(${target} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../include)
		# libstdc++ 16 asserts at -O0, and its assertion handler needs the
		# unwinder: a bare-metal image turns the assertions off.
		target_compile_definitions(${target} PRIVATE KYMO_RING_SLOTS=16 _GLIBCXX_NO_ASSERTIONS)
		target_compile_options(${target} PRIVATE -mcpu=${cpu} -mthumb -${level}
			-g -Wall -Wextra -Werror
			-ffunction-sections -fdata-sections
			-fno-exceptions -fno-rtti -fno-threadsafe-statics)
		target_link_options(${target} PRIVATE -mcpu=${cpu} -mthumb
			-nostdlib -nostartfiles -Wl,--gc-sections -T ${script})
		if(missing)
			add_test(NAME ${target} COMMAND ${CMAKE_COMMAND} -E echo "missing: ${missing}")
			set_tests_properties(${target} PROPERTIES WILL_FAIL ON)
		else()
			add_test(NAME ${target}
				COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/run_qemu.py
					--qemu ${KYMO_QEMU} --machine ${machine} --image $<TARGET_FILE:${target}>
					--icount-shift ${KYMO_ICOUNT_SHIFT})
		endif()
	endforeach()
endforeach()

