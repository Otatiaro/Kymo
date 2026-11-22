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
find_program(KYMO_SIZE arm-none-eabi-size HINTS "${bin}" REQUIRED)

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

# The QEMU machine and linker script of each core.
set(machine_cortex-m33 mps2-an505)
set(script_cortex-m33 ${CMAKE_CURRENT_SOURCE_DIR}/qemu/armv8m.ld)
set(machine_cortex-m4 mps2-an386)
set(script_cortex-m4 ${CMAKE_CURRENT_SOURCE_DIR}/qemu/armv7m.ld)

# An image for QEMU, at an optimisation level, with link-time optimisation
# when it ends in -lto (Os-lto):
#   kymo_qemu_image(<name> <cpu> <level> SOURCES <files> [DEFINITIONS <defs>])
function(kymo_qemu_image name cpu level)
	cmake_parse_arguments(PARSE_ARGV 3 arg "" "" "SOURCES;DEFINITIONS")
	set(lto "")
	if(level MATCHES "-lto$")
		string(REPLACE "-lto" "" level ${level})
		set(lto -${level} -flto)
	endif()
	set(script ${script_${cpu}})
	add_executable(${name} ${arg_SOURCES} qemu/startup.cpp)
	set_target_properties(${name} PROPERTIES SUFFIX ".elf" LINK_DEPENDS ${script})
	target_include_directories(${name} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../include)
	# libstdc++ 16 asserts at -O0, and its assertion handler needs the
	# unwinder: a bare-metal image turns the assertions off.
	target_compile_definitions(${name} PRIVATE KYMO_RING_SLOTS=16 _GLIBCXX_NO_ASSERTIONS ${arg_DEFINITIONS})
	target_compile_options(${name} PRIVATE -mcpu=${cpu} -mthumb -${level}
		-g -Wall -Wextra -Werror
		-ffunction-sections -fdata-sections
		-fno-exceptions -fno-rtti -fno-threadsafe-statics ${lto})
	target_link_options(${name} PRIVATE -mcpu=${cpu} -mthumb ${lto}
		-nostdlib -nostartfiles -Wl,--gc-sections -T ${script})
endfunction()

# A test that runs a Python script, or fails saying what is missing:
#   kymo_qemu_command(<name> <script> <arguments>...)
function(kymo_qemu_command name)
	if(missing)
		add_test(NAME ${name} COMMAND ${CMAKE_COMMAND} -E echo "missing: ${missing}")
		set_tests_properties(${name} PROPERTIES WILL_FAIL ON)
	else()
		add_test(NAME ${name} COMMAND ${Python3_EXECUTABLE} ${ARGN})
	endif()
endfunction()

# An image that reports its own verdict, run under QEMU:
#   kymo_qemu_test(<name> <cpu> <level> SOURCES <files> [DEFINITIONS <defs>] [ICOUNT])
function(kymo_qemu_test name cpu level)
	cmake_parse_arguments(PARSE_ARGV 3 arg "ICOUNT" "" "SOURCES;DEFINITIONS")
	kymo_qemu_image(${name} ${cpu} ${level} SOURCES ${arg_SOURCES} DEFINITIONS ${arg_DEFINITIONS})
	set(icount "")
	if(arg_ICOUNT)
		set(icount --icount-shift ${KYMO_ICOUNT_SHIFT})
	endif()
	kymo_qemu_command(${name} ${CMAKE_CURRENT_SOURCE_DIR}/run_qemu.py
		--qemu ${KYMO_QEMU} --machine ${machine_${cpu}} --image $<TARGET_FILE:${name}> ${icount})
endfunction()

foreach(cpu cortex-m33 cortex-m4)
	foreach(level O0 Os O2)
		# Lock-free writes: an interrupt that emits, at every instruction of an emit.
		kymo_qemu_test(emit-preemption-${cpu}-${level} ${cpu} ${level} ICOUNT
			SOURCES emit/preemption.cpp qemu/preempt.cpp)
		# The control word: value from reset, filter by family, freeze, and
		# enable() and disable() interrupted at every instruction.
		kymo_qemu_test(control-${cpu}-${level} ${cpu} ${level} ICOUNT
			SOURCES control/control.cpp qemu/preempt.cpp
			DEFINITIONS KYMO_CONTROL_DEFAULT=0xffffffbfu)
	endforeach()
endforeach()

# What the host reads from an image, checked against the image itself. Needs
# the host, built from host/ (cargo build --release):
# - layout: the event layouts from the DWARF (kymo events) against what the
#   compiler laid out;
# - decode: a copy of the ring (kymo decode) against the events written to it.
find_program(KYMO_HOST kymo HINTS ${CMAKE_CURRENT_SOURCE_DIR}/../../host/target/release)
foreach(test layout decode)
	foreach(cpu cortex-m33 cortex-m4)
		foreach(level O0 Os Os-lto)
			set(name ${test}-${cpu}-${level})
			kymo_qemu_image(${name} ${cpu} ${level} SOURCES ${test}/${test}.cpp)
			if(KYMO_HOST)
				kymo_qemu_command(${name}
					${CMAKE_CURRENT_SOURCE_DIR}/${test}/${test}.py --qemu ${KYMO_QEMU} --machine ${machine_${cpu}}
					--image $<TARGET_FILE:${name}> --host ${KYMO_HOST})
			else()
				add_test(NAME ${name} COMMAND ${CMAKE_COMMAND} -E echo "missing: KYMO_HOST")
				set_tests_properties(${name} PROPERTIES WILL_FAIL ON)
			endif()
		endforeach()
	endforeach()
endforeach()

# What an emit costs, in instructions executed under QEMU, against the
# threshold: 50 for a one-slot event, 15 for each slot more. QEMU counts
# instructions; cycles are measured on hardware. Optimised builds only: -O0
# is not what ships.
set(KYMO_COST_EVENT 50 CACHE STRING "Most instructions a one-slot event may cost")
set(KYMO_COST_SLOT 15 CACHE STRING "Most instructions each slot more may cost")
foreach(cpu cortex-m33 cortex-m4)
	foreach(level Os O2)
		kymo_qemu_image(cost-${cpu}-${level} ${cpu} ${level} SOURCES cost/cost.cpp)
		kymo_qemu_image(cost-baseline-${cpu}-${level} ${cpu} ${level} SOURCES cost/cost.cpp
			DEFINITIONS KYMO_COST_BASELINE)
		kymo_qemu_command(cost-${cpu}-${level}
			${CMAKE_CURRENT_SOURCE_DIR}/cost.py --qemu ${KYMO_QEMU} --machine ${machine_${cpu}}
			--nm ${KYMO_NM} --size ${KYMO_SIZE}
			--image $<TARGET_FILE:cost-${cpu}-${level}>
			--baseline $<TARGET_FILE:cost-baseline-${cpu}-${level}>
			--max-event ${KYMO_COST_EVENT} --max-slot ${KYMO_COST_SLOT})
	endforeach()
endforeach()
