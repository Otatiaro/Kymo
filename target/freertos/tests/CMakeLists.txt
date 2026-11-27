cmake_minimum_required(VERSION 3.25)

# The FreeRTOS adapter, on a test application (application.cpp) run under
# QEMU on the Cortex-M4 (port ARM_CM4F) and the Cortex-M33 (port
# ARM_CM33_NTZ), recorded by the host and checked (trace/trace.py):
#   cmake -S target/freertos/tests -B build/freertos -G Ninja \
#         -DCMAKE_TOOLCHAIN_FILE=$PWD/target/cmake/arm-none-eabi.cmake \
#         -DKYMO_FREERTOS_DIR=<FreeRTOS-Kernel's sources>
#   cmake --build build/freertos && ctest --test-dir build/freertos
project(kymo_freertos_tests C CXX)
enable_testing()

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

set(KYMO_FREERTOS_DIR "" CACHE PATH "FreeRTOS-Kernel's sources, at the commit the CI pins")
if(NOT EXISTS "${KYMO_FREERTOS_DIR}/tasks.c")
	message(FATAL_ERROR "KYMO_FREERTOS_DIR must name FreeRTOS-Kernel's sources (no tasks.c in '${KYMO_FREERTOS_DIR}')")
endif()

set(target_dir ${CMAKE_CURRENT_SOURCE_DIR}/../..)
set(qemu_dir ${target_dir}/tests/qemu)
set(portable ${KYMO_FREERTOS_DIR}/portable/GCC)

# Per core: the QEMU machine, the linker script, the port and its sources,
# the floating point the port expects.
set(machine_cortex-m4 mps2-an386)
set(script_cortex-m4 ${qemu_dir}/armv7m.ld)
set(port_cortex-m4 ${portable}/ARM_CM4F)
set(port_sources_cortex-m4 ${portable}/ARM_CM4F/port.c)
set(float_cortex-m4 -mfloat-abi=hard -mfpu=fpv4-sp-d16)

set(machine_cortex-m33 mps2-an505)
set(script_cortex-m33 ${qemu_dir}/armv8m.ld)
set(port_cortex-m33 ${portable}/ARM_CM33_NTZ/non_secure)
set(port_sources_cortex-m33 ${port_cortex-m33}/port.c ${port_cortex-m33}/portasm.c)
set(float_cortex-m33 -mfloat-abi=hard -mfpu=fpv5-sp-d16)

find_program(KYMO_QEMU qemu-system-arm HINTS "C:/Program Files/qemu")
find_program(KYMO_HOST kymo HINTS ${target_dir}/../host/target/release)
find_package(Python3 COMPONENTS Interpreter)
set(missing "")
foreach(tool KYMO_QEMU KYMO_HOST Python3_EXECUTABLE)
	if(NOT ${tool})
		list(APPEND missing ${tool})
	endif()
endforeach()

# The kernel, built as its sources are, without Kymo's warnings as errors;
# `untraced` defines KYMO_UNTRACED, which leaves the adapter out of
# FreeRTOSConfig.h.
function(kymo_freertos_kernel kernel cpu untraced)
	add_library(${kernel} STATIC
		${KYMO_FREERTOS_DIR}/tasks.c ${KYMO_FREERTOS_DIR}/queue.c ${KYMO_FREERTOS_DIR}/list.c
		${KYMO_FREERTOS_DIR}/event_groups.c ${KYMO_FREERTOS_DIR}/stream_buffer.c
		${port_sources_${cpu}})
	target_include_directories(${kernel} PUBLIC
		${CMAKE_CURRENT_SOURCE_DIR} ${target_dir}/freertos/include
		${KYMO_FREERTOS_DIR}/include ${port_${cpu}})
	target_compile_options(${kernel} PRIVATE
		-mcpu=${cpu} -mthumb ${float_${cpu}} -Os -g -ffunction-sections -fdata-sections)
	if(untraced)
		target_compile_definitions(${kernel} PUBLIC KYMO_UNTRACED)
	endif()
endfunction()

# An image on a kernel, linked by a script that lays out the sections of
# every QEMU image: the machine's, or the board's.
function(kymo_freertos_image image cpu script kernel)
	add_executable(${image} ${ARGN} ${qemu_dir}/startup.cpp)
	set_target_properties(${image} PROPERTIES SUFFIX ".elf"
		LINK_DEPENDS "${script};${qemu_dir}/sections.ld")
	target_include_directories(${image} PRIVATE ${target_dir}/include ${qemu_dir})
	target_link_libraries(${image} PRIVATE ${kernel})
	set(cpu_options -mcpu=${cpu} -mthumb ${float_${cpu}})
	target_compile_options(${image} PRIVATE ${cpu_options} -Os
		-g -Wall -Wextra -Werror
		-ffunction-sections -fdata-sections
		-fno-exceptions -fno-rtti -fno-threadsafe-statics)
	target_link_options(${image} PRIVATE ${cpu_options}
		-nostdlib -nostartfiles -Wl,--gc-sections
		-L${qemu_dir} -T ${script})
endfunction()

# The bench's two images, `<prefix>-untraced` and `<prefix>-traced`, each on
# a kernel of its own, with `definitions` on both.
function(kymo_freertos_bench prefix cpu script definitions)
	kymo_freertos_kernel(${prefix}-kernel-untraced ${cpu} ON)
	kymo_freertos_kernel(${prefix}-kernel-traced ${cpu} OFF)
	foreach(variant untraced traced)
		target_compile_definitions(${prefix}-kernel-${variant} PUBLIC ${definitions})
	endforeach()
	kymo_freertos_image(${prefix}-untraced ${cpu} ${script} ${prefix}-kernel-untraced bench/bench.cpp)
	kymo_freertos_image(${prefix}-traced ${cpu} ${script} ${prefix}-kernel-traced
		${target_dir}/freertos/kymo_freertos.cpp bench/bench.cpp)
	foreach(variant untraced traced)
		target_include_directories(${prefix}-${variant} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})
	endforeach()
endfunction()

foreach(cpu cortex-m4 cortex-m33)
	set(kernel freertos-${cpu})
	kymo_freertos_kernel(${kernel} ${cpu} OFF)
	set(image freertos-${cpu}-Os)
	kymo_freertos_image(${image} ${cpu} ${script_${cpu}} ${kernel}
		${target_dir}/freertos/kymo_freertos.cpp application.cpp)

	# The cost bench, untraced and traced: bench/bench.cpp, on kernels of
	# their own built with KYMO_BENCH, which leaves the assertions out.
	kymo_freertos_bench(freertos-bench-${cpu} ${cpu} ${script_${cpu}} "KYMO_BENCH")

	# The fault event and its call levels: fault/fault.cpp checks what the
	# adapter wrote to the ring, so only QEMU runs it.
	kymo_freertos_image(freertos-fault-${cpu} ${cpu} ${script_${cpu}} ${kernel}
		${target_dir}/freertos/kymo_freertos.cpp fault/fault.cpp)
	target_include_directories(freertos-fault-${cpu} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR} ${target_dir}/tests/include)
	if(KYMO_QEMU)
		add_test(NAME freertos-fault-${cpu}
			COMMAND ${KYMO_QEMU} -machine ${machine_${cpu}} -kernel $<TARGET_FILE:freertos-fault-${cpu}>
				-nographic -semihosting-config enable=on,target=native -monitor none -serial none)
		set_tests_properties(freertos-fault-${cpu} PROPERTIES PASS_REGULAR_EXPRESSION "RESULT: PASS" TIMEOUT 60)
	endif()

	if(missing)
		add_test(NAME ${image} COMMAND ${CMAKE_COMMAND} -E echo "missing: ${missing}")
		set_tests_properties(${image} PROPERTIES WILL_FAIL ON)
	else()
		add_test(NAME ${image}
			COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/trace/trace.py
				--qemu ${KYMO_QEMU} --machine ${machine_${cpu}}
				--image $<TARGET_FILE:${image}> --host ${KYMO_HOST})
	endif()
endforeach()

# The same application on a NUCLEO-H563ZI (KYMO_BOARD), recorded through its
# ST-Link and checked (board/board.py): with -DKYMO_FREERTOS_BOARD=<ST-Link
# serial number>. Needs probe-rs to flash it, and arm-none-eabi-nm to find
# the verdict. Out of the CI: it needs the board.
set(KYMO_FREERTOS_BOARD "" CACHE STRING "The serial number of the NUCLEO-H563ZI's ST-Link, to build and record board/")
if(KYMO_FREERTOS_BOARD)
	kymo_freertos_kernel(freertos-h563 cortex-m33 OFF)
	target_compile_definitions(freertos-h563 PUBLIC KYMO_BOARD)
	kymo_freertos_image(freertos-h563-Os cortex-m33 ${CMAKE_CURRENT_SOURCE_DIR}/board/h563.ld freertos-h563
		${target_dir}/freertos/kymo_freertos.cpp application.cpp)
	find_program(KYMO_PROBE_RS probe-rs)
	add_test(NAME freertos_board_h563
		COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/board/board.py
			--image $<TARGET_FILE:freertos-h563-Os> --host ${KYMO_HOST}
			--probe-rs ${KYMO_PROBE_RS} --probe-id ${KYMO_FREERTOS_BOARD} --nm ${CMAKE_NM})
	set_tests_properties(freertos_board_h563 PROPERTIES TIMEOUT 120)

	# The bench on the board, at 250 MHz, timed in cycles: what OpSy's and
	# Zephyr's bench images print on the same board, for the comparison.
	#   probe-rs run --chip STM32H563ZITx freertos-bench-h563-untraced.elf
	kymo_freertos_bench(freertos-bench-h563 cortex-m33 ${CMAKE_CURRENT_SOURCE_DIR}/board/h563.ld
		"KYMO_BENCH;KYMO_BENCH_BOARD")
endif()
