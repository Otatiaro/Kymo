cmake_minimum_required(VERSION 3.25)

# The OpSy adapter, on OpSy's own QEMU suite: the suite's images built with
# traced/opsy_hooks.hpp on their include path, so that every hook OpSy calls
# while the suite runs emits its event, through the tracer of opsy_trace.hpp. One core per build tree, like OpSy's:
#   cmake -S target/opsy/tests -B build/opsy-m33 -G Ninja \
#         -DCMAKE_TOOLCHAIN_FILE=$PWD/target/cmake/arm-none-eabi.cmake \
#         -DKYMO_OPSY_DIR=<OpSy's sources> -DOPSY_TARGET=m33 "-DOPSY_OPTIMISATION=-Os;-DNDEBUG"
#   cmake --build build/opsy-m33 && ctest --test-dir build/opsy-m33
project(kymo_opsy_tests CXX ASM)
enable_testing()

set(KYMO_OPSY_DIR "" CACHE PATH "OpSy's sources, at the commit the CI pins")
if(NOT EXISTS "${KYMO_OPSY_DIR}/hooks.hpp")
	message(FATAL_ERROR "KYMO_OPSY_DIR must name OpSy's sources (no hooks.hpp in '${KYMO_OPSY_DIR}')")
endif()

add_subdirectory(${KYMO_OPSY_DIR}/tests/qemu opsy)

set(kymo_include
	${CMAKE_CURRENT_SOURCE_DIR}/../../include
	${CMAKE_CURRENT_SOURCE_DIR}/../include
	${CMAKE_CURRENT_SOURCE_DIR})

foreach(image opsy_qemu_tests opsy_qemu_start_check opsy_qemu_before_start)
	target_include_directories(${image} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/traced ${kymo_include})
endforeach()

# The fault image, traced: the adapter's events and the fault recording OpSy's
# test_faults.cpp checks, ahead of OpSy's own fault hooks.
target_include_directories(opsy_qemu_faults BEFORE PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/faults ${kymo_include})

# The bench, traced: its markers and the adapter's events together, ahead of
# OpSy's own bench hooks. bench/overhead.py then holds what tracing adds to
# each path against its record.
target_include_directories(opsy_qemu_bench BEFORE PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/bench ${kymo_include})

# OpSy's suite with Kymo's directories on its include path and an event of
# its own, but no opsy_hooks.hpp: the program that forgot to declare its
# hooks. The host decodes it and warns (opsy_unhooked below).
get_target_property(suite_sources opsy_qemu_tests SOURCES)
add_executable(opsy_untraced ${suite_sources} ${CMAKE_CURRENT_SOURCE_DIR}/untraced/application.cpp)
foreach(variable OPSY_CPU_FLAGS OPSY_ROOT OPSY_LINKER_SCRIPT_PATH)
	get_directory_property(${variable} DIRECTORY ${KYMO_OPSY_DIR}/tests/qemu DEFINITION ${variable})
endforeach()
opsy_qemu_image(opsy_untraced)
target_compile_options(opsy_untraced PRIVATE -g ${OPSY_OPTIMISATION})
target_link_options(opsy_untraced PRIVATE -Wl,--undefined=start_application)
target_include_directories(opsy_untraced PRIVATE ${KYMO_OPSY_DIR}/tests/qemu ${kymo_include})

# The adapter's events read back from the ring, as a case of OpSy's suite.
target_sources(opsy_qemu_tests PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/ring_check.cpp)

# OpSy's suite recorded by the host through QEMU's monitor, and the trace
# checked against what the scheduler does (trace/trace.py). It needs the host
# built (cargo build --release in host/), QEMU and Python, or fails saying
# which is missing.
get_directory_property(machine DIRECTORY ${KYMO_OPSY_DIR}/tests/qemu DEFINITION OPSY_QEMU_MACHINE)
find_program(KYMO_QEMU qemu-system-arm HINTS "C:/Program Files/qemu")
find_program(KYMO_HOST kymo HINTS ${CMAKE_CURRENT_SOURCE_DIR}/../../../host/target/release)
find_package(Python3 COMPONENTS Interpreter)
set(missing "")
foreach(tool KYMO_QEMU KYMO_HOST Python3_EXECUTABLE)
	if(NOT ${tool})
		list(APPEND missing ${tool})
	endif()
endforeach()
if(missing)
	add_test(NAME opsy_trace_${OPSY_TARGET} COMMAND ${CMAKE_COMMAND} -E echo "missing: ${missing}")
	set_tests_properties(opsy_trace_${OPSY_TARGET} PROPERTIES WILL_FAIL ON)
else()
	add_test(NAME opsy_trace_${OPSY_TARGET}
		COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/trace/trace.py
			--qemu ${KYMO_QEMU} --machine ${machine}
			--image $<TARGET_FILE:opsy_qemu_tests> --host ${KYMO_HOST})

	# The host's warning on an image that runs OpSy without Kymo's hooks: one
	# with an event of its own decodes and warns; one with no event at all
	# fails saying why; the traced suite is not warned about.
	add_test(NAME opsy_unhooked_${OPSY_TARGET} COMMAND ${KYMO_HOST} events $<TARGET_FILE:opsy_untraced>)
	set_tests_properties(opsy_unhooked_${OPSY_TARGET} PROPERTIES
		PASS_REGULAR_EXPRESSION "warning: the image runs OpSy without Kymo's hooks"
		FAIL_REGULAR_EXPRESSION "error:")
	add_test(NAME opsy_unhooked_bare_${OPSY_TARGET} COMMAND ${KYMO_HOST} events $<TARGET_FILE:opsy_qemu_notify_check>)
	set_tests_properties(opsy_unhooked_bare_${OPSY_TARGET} PROPERTIES
		PASS_REGULAR_EXPRESSION "error: .*no event type ids \\(\\.kymo\\.ids\\). the image runs OpSy without Kymo's hooks")
	add_test(NAME opsy_hooked_${OPSY_TARGET} COMMAND ${KYMO_HOST} events $<TARGET_FILE:opsy_qemu_tests>)
	set_tests_properties(opsy_hooked_${OPSY_TARGET} PROPERTIES FAIL_REGULAR_EXPRESSION "warning:")
endif()

# An OpSy program on a NUCLEO-H563ZI (board/board.cpp), recorded through its
# ST-Link and checked (board/board.py): with -DKYMO_OPSY_BOARD=<ST-Link serial
# number>, on an M33 build without NDEBUG. Needs probe-rs to flash it.
set(KYMO_OPSY_BOARD "" CACHE STRING "The serial number of the NUCLEO-H563ZI's ST-Link, to build and record board/")
if(KYMO_OPSY_BOARD)
	if(NOT OPSY_TARGET STREQUAL "m33")
		message(FATAL_ERROR "board/ runs on the STM32H563, a Cortex-M33: OPSY_TARGET must be m33")
	endif()
	add_executable(opsy_board
		${KYMO_OPSY_DIR}/scheduler.cpp
		${KYMO_OPSY_DIR}/tests/qemu/startup.cpp
		${CMAKE_CURRENT_SOURCE_DIR}/board/board.cpp)
	get_directory_property(OPSY_CPU_FLAGS DIRECTORY ${KYMO_OPSY_DIR}/tests/qemu DEFINITION OPSY_CPU_FLAGS)
	get_directory_property(OPSY_ROOT DIRECTORY ${KYMO_OPSY_DIR}/tests/qemu DEFINITION OPSY_ROOT)
	set(OPSY_LINKER_SCRIPT_PATH ${KYMO_OPSY_DIR}/tests/qemu/bench/stm32h563zi.ld)
	opsy_qemu_image(opsy_board)
	# What OpSy's directory adds to its own images: debug information and the
	# optimisation level.
	target_compile_options(opsy_board PRIVATE -g ${OPSY_OPTIMISATION})
	target_include_directories(opsy_board PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/traced ${KYMO_OPSY_DIR}/tests/qemu ${kymo_include})
	find_program(KYMO_PROBE_RS probe-rs)
	add_test(NAME opsy_board_h563
		COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/board/board.py
			--image $<TARGET_FILE:opsy_board> --host ${KYMO_HOST}
			--probe-rs ${KYMO_PROBE_RS} --probe-id ${KYMO_OPSY_BOARD})
	set_tests_properties(opsy_board_h563 PROPERTIES TIMEOUT 120)
endif()
