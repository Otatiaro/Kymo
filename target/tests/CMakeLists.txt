cmake_minimum_required(VERSION 3.25)

# Target-side tests that need only the toolchain. Configure with it, then run
# them with ctest:
#   cmake -S target/tests -B build/tests -G Ninja \
#         -DCMAKE_TOOLCHAIN_FILE=$PWD/target/cmake/arm-none-eabi.cmake
#   cmake --build build/tests && ctest --test-dir build/tests
project(kymo_tests CXX C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)
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
	foreach(variant Og Os O2 Os-lto)
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
	set_target_properties(${name} PROPERTIES SUFFIX ".elf"
		LINK_DEPENDS "${script};${CMAKE_CURRENT_SOURCE_DIR}/qemu/sections.ld")
	target_include_directories(${name} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../include)
	target_compile_definitions(${name} PRIVATE ${arg_DEFINITIONS})
	if(COMMAND kymo_clang_headers)
		kymo_clang_headers(${cpu} headers)
		target_compile_options(${name} PRIVATE ${headers})
	endif()
	target_compile_options(${name} PRIVATE -mcpu=${cpu} -mthumb -${level}
		-g -Wall -Wextra -Werror
		-ffunction-sections -fdata-sections
		$<$<COMPILE_LANGUAGE:CXX>:-fno-exceptions -fno-rtti -fno-threadsafe-statics> ${lto})
	target_link_options(${name} PRIVATE -mcpu=${cpu} -mthumb ${lto}
		-nostdlib -nostartfiles -Wl,--gc-sections
		-L${CMAKE_CURRENT_SOURCE_DIR}/qemu -T ${script})
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
	foreach(level Og Os O2)
		# Lock-free writes: an interrupt that emits, at every instruction of an emit.
		kymo_qemu_test(emit-preemption-${cpu}-${level} ${cpu} ${level} ICOUNT
			SOURCES emit/preemption.cpp qemu/preempt.cpp)
		# A writer preempted for a full lap: what the host takes as intact is,
		# and the writer counts itself lapped.
		kymo_qemu_test(emit-lap-${cpu}-${level} ${cpu} ${level} ICOUNT
			SOURCES emit/lap.cpp qemu/preempt.cpp)
		# The control word: value from reset, filter by family, freeze, and
		# enable() and disable() interrupted at every instruction.
		kymo_qemu_test(control-${cpu}-${level} ${cpu} ${level} ICOUNT
			SOURCES control/control.cpp qemu/preempt.cpp)
		# The DWT data trace: the comparator chosen against real chips' registers
		# (at compile time), and anchor() and watch() refusing under QEMU, which
		# models no comparator.
		kymo_qemu_test(dwt-${cpu}-${level} ${cpu} ${level} SOURCES dwt/dwt.cpp)
	endforeach()
endforeach()

# What the host reads from an image, checked against the image itself. Needs
# the host, built from host/ (cargo build --release):
# - layout: the event layouts from the DWARF (kymo events) against what the
#   compiler laid out;
# - decode: a copy of the ring (kymo decode) against the events written to it;
# - record: what the host records of a running image through QEMU's monitor
#   (kymo record), against the values each event carries;
# - symbols: pointer fields named from the ELF by kymo record, against the
#   functions and objects the image points at;
# - text: events whose types declare a text, written by kymo decode with their
#   fields' values (std::format's specifications), against what the image
#   says; text-bad, a text naming a field its event lacks, refused;
# - durations: begins and ends paired by kymo decode, stats and check, against
#   the durations the image measures;
# - values: the numeric fields of application events read as curves by kymo
#   stats and check, against the values the image emits.
find_program(KYMO_HOST kymo HINTS ${CMAKE_CURRENT_SOURCE_DIR}/../../host/target/release)
foreach(test layout decode record symbols text text-bad durations values)
	string(REGEX REPLACE "-bad$" "" source ${test})
	set(definitions "")
	set(option "")
	if(test STREQUAL text-bad)
		set(definitions KYMO_TEST_BAD_TEXT)
		set(option --bad)
	endif()
	foreach(cpu cortex-m33 cortex-m4)
		foreach(level Og Os Os-lto)
			set(name ${test}-${cpu}-${level})
			kymo_qemu_image(${name} ${cpu} ${level} SOURCES ${source}/${source}.cpp DEFINITIONS ${definitions})
			if(KYMO_HOST)
				kymo_qemu_command(${name}
					${CMAKE_CURRENT_SOURCE_DIR}/${source}/${source}.py --qemu ${KYMO_QEMU} --machine ${machine_${cpu}}
					--image $<TARGET_FILE:${name}> --host ${KYMO_HOST} ${option})
			else()
				add_test(NAME ${name} COMMAND ${CMAKE_COMMAND} -E echo "missing: KYMO_HOST")
				set_tests_properties(${name} PROPERTIES WILL_FAIL ON)
			endif()
		endforeach()
	endforeach()
endforeach()

# - no-text: the texts of the durations image, an application that only emits
#   its events, are in its DWARF, never in what the target loads
#   (objcopy -O binary). The text image prints its texts, so holds them.
foreach(cpu cortex-m33 cortex-m4)
	foreach(level Og Os Os-lto)
		set(name no-text-${cpu}-${level})
		if(KYMO_HOST AND Python3_EXECUTABLE)
			add_test(NAME ${name} COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/no_text.py
				--host ${KYMO_HOST} --objcopy ${CMAKE_OBJCOPY} --image $<TARGET_FILE:durations-${cpu}-${level}>)
		else()
			add_test(NAME ${name} COMMAND ${CMAKE_COMMAND} -E echo "missing: KYMO_HOST or Python")
			set_tests_properties(${name} PROPERTIES WILL_FAIL ON)
		endif()
	endforeach()
endforeach()

# - interrupts: the device interrupts named from the DWARF's IRQn_Type, in
#   SvdGenerator's convention, in a vendor's CMSIS one, or in both (none
#   named, a warning); then with an SVD file naming those the DWARF does not.
#   kymo events reads the image, which does not run.
foreach(convention svdgenerator cmsis both)
	set(sources interrupts/interrupts.cpp)
	set(definitions "")
	if(NOT convention STREQUAL cmsis)
		list(APPEND definitions KYMO_TEST_SVDGENERATOR)
	endif()
	if(NOT convention STREQUAL svdgenerator)
		list(APPEND sources interrupts/cmsis.c)
		list(APPEND definitions KYMO_TEST_CMSIS)
	endif()
	foreach(cpu cortex-m33 cortex-m4)
		foreach(level Og Os Os-lto)
			set(image interrupts-${convention}-${cpu}-${level})
			kymo_qemu_image(${image} ${cpu} ${level} SOURCES ${sources} DEFINITIONS ${definitions})
			foreach(svd none device)
				set(name ${image})
				set(option "")
				if(svd STREQUAL device)
					set(name ${image}-svd)
					set(option --svd ${CMAKE_CURRENT_SOURCE_DIR}/interrupts/device.svd)
				endif()
				if(KYMO_HOST AND Python3_EXECUTABLE)
					add_test(NAME ${name} COMMAND ${Python3_EXECUTABLE}
						${CMAKE_CURRENT_SOURCE_DIR}/interrupts/interrupts.py --host ${KYMO_HOST}
						--image $<TARGET_FILE:${image}> --convention ${convention} ${option})
				else()
					add_test(NAME ${name} COMMAND ${CMAKE_COMMAND} -E echo "missing: KYMO_HOST or Python")
					set_tests_properties(${name} PROPERTIES WILL_FAIL ON)
				endif()
			endforeach()
		endforeach()
	endforeach()
endforeach()

# - record-killed: the host killed while it records, amid the traffic or
#   once the target is stopped, its capture read back up to where it was cut.
foreach(cpu cortex-m33 cortex-m4)
	kymo_qemu_image(record-killed-${cpu}-Os ${cpu} Os SOURCES record/record.cpp)
	foreach(when running paused)
		set(name record-killed-${when}-${cpu}-Os)
		if(KYMO_HOST)
			kymo_qemu_command(${name}
				${CMAKE_CURRENT_SOURCE_DIR}/record/record.py --qemu ${KYMO_QEMU} --machine ${machine_${cpu}}
				--image $<TARGET_FILE:record-killed-${cpu}-Os> --host ${KYMO_HOST} --killed ${when})
		else()
			add_test(NAME ${name} COMMAND ${CMAKE_COMMAND} -E echo "missing: KYMO_HOST")
			set_tests_properties(${name} PROPERTIES WILL_FAIL ON)
		endif()
	endforeach()
endforeach()

# - channels: two rings in one image, each event in the ring of its family,
#   each ring read by its name; without a name, the host lists both.
foreach(cpu cortex-m33 cortex-m4)
	set(name channels-${cpu}-Os)
	kymo_qemu_image(${name} ${cpu} Os SOURCES channels/channels.cpp)
	if(KYMO_HOST)
		kymo_qemu_command(${name}
			${CMAKE_CURRENT_SOURCE_DIR}/channels/channels.py --qemu ${KYMO_QEMU} --machine ${machine_${cpu}}
			--image $<TARGET_FILE:${name}> --host ${KYMO_HOST})
	else()
		add_test(NAME ${name} COMMAND ${CMAKE_COMMAND} -E echo "missing: KYMO_HOST")
		set_tests_properties(${name} PROPERTIES WILL_FAIL ON)
	endif()
endforeach()

# A family with no route does not compile once one of its events is emitted;
# with its route, the same source compiles.
foreach(variant routed unrouted)
	set(definitions "")
	if(variant STREQUAL routed)
		set(definitions -DKYMO_TEST_ROUTED)
	endif()
	add_test(NAME channels-${variant}
		COMMAND ${CMAKE_CXX_COMPILER} -std=c++17 -mcpu=cortex-m4 -mthumb -fsyntax-only
			-Wall -Wextra -Werror ${definitions}
			-I ${CMAKE_CURRENT_SOURCE_DIR}/../include ${CMAKE_CURRENT_SOURCE_DIR}/channels/unrouted.cpp)
endforeach()
set_tests_properties(channels-unrouted PROPERTIES
	PASS_REGULAR_EXPRESSION "error: [^\n]*on<kymo::family::scheduling>")

# - record-lap: writers held for more than a lap while the host records,
#   which follows (it reads each held writer before the lap) or falls behind
#   (the lap comes between two of its reads).
foreach(cpu cortex-m33 cortex-m4)
	foreach(lap follows behind)
		if(lap STREQUAL follows)
			set(hold_pause 1000000)
		else()
			set(hold_pause 0)
		endif()
		set(name record-lap-${lap}-${cpu}-Os)
		kymo_qemu_image(${name} ${cpu} Os SOURCES record/lap.cpp qemu/preempt.cpp
			DEFINITIONS KYMO_TEST_HOLD_PAUSE=${hold_pause})
		if(KYMO_HOST)
			kymo_qemu_command(${name}
				${CMAKE_CURRENT_SOURCE_DIR}/record/record.py --qemu ${KYMO_QEMU} --machine ${machine_${cpu}}
				--image $<TARGET_FILE:${name}> --host ${KYMO_HOST} --lap ${lap} --icount-shift ${KYMO_ICOUNT_SHIFT})
			# Whether the host follows depends on how fast it reads: alone,
			# not slowed down by tests run alongside.
			set_tests_properties(${name} PROPERTIES RUN_SERIAL ON)
		else()
			add_test(NAME ${name} COMMAND ${CMAKE_COMMAND} -E echo "missing: KYMO_HOST")
			set_tests_properties(${name} PROPERTIES WILL_FAIL ON)
		endif()
	endforeach()
endforeach()

# What an emit costs, in instructions executed under QEMU, held against the
# record of each CPU and level (cost/cost-<cpu>-<level>.json), the disabled
# path included. QEMU counts instructions; cycles are measured on hardware.
# The builds that ship: -Os and -O2, not the debug build. Clang's builds
# (cmake/arm-none-eabi-clang.cmake) have no record: they hold the
# application's events at the cost of the plain emit, nothing more.
set(KYMO_COST_TOLERANCE 2 CACHE STRING "Instructions a case may cost over its record")
foreach(cpu cortex-m33 cortex-m4)
	foreach(level Os O2)
		set(record --expected ${CMAKE_CURRENT_SOURCE_DIR}/cost/cost-${cpu}-${level}.json)
		if(CMAKE_CXX_COMPILER_ID STREQUAL "Clang")
			set(record "")
		endif()
		kymo_qemu_image(cost-${cpu}-${level} ${cpu} ${level} SOURCES cost/cost.cpp)
		kymo_qemu_image(cost-baseline-${cpu}-${level} ${cpu} ${level} SOURCES cost/cost.cpp
			DEFINITIONS KYMO_COST_BASELINE)
		kymo_qemu_command(cost-${cpu}-${level}
			${CMAKE_CURRENT_SOURCE_DIR}/cost.py --qemu ${KYMO_QEMU} --machine ${machine_${cpu}}
			--nm ${KYMO_NM} --size ${KYMO_SIZE}
			--image $<TARGET_FILE:cost-${cpu}-${level}>
			--baseline $<TARGET_FILE:cost-baseline-${cpu}-${level}>
			${record}
			--tolerance ${KYMO_COST_TOLERANCE})
	endforeach()
endforeach()
