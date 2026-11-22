cmake_minimum_required(VERSION 3.25)

# Throughput bench images, one per board. Configure with the toolchain file:
#   cmake -S target/bench -B build/bench -G Ninja \
#         -DCMAKE_TOOLCHAIN_FILE=target/cmake/arm-none-eabi.cmake
project(kymo_bench CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

set(KYMO_BENCH_RING_SLOTS 1024 CACHE STRING "Slots in the bench ring (power of two)")

# board  core        core clock after reset
#  h563  cortex-m33  32 MHz (HSI 64 MHz, HSIDIV /2)
#  h523  cortex-m33  32 MHz
#  f401  cortex-m4   16 MHz (HSI)
#  f469  cortex-m4   16 MHz
set(boards h563 h523 f401 f469)
set(cpu_h563 cortex-m33)
set(cpu_h523 cortex-m33)
set(cpu_f401 cortex-m4)
set(cpu_f469 cortex-m4)
set(clock_h563 32000000)
set(clock_h523 32000000)
set(clock_f401 16000000)
set(clock_f469 16000000)

foreach(board IN LISTS boards)
	set(target kymo-bench-${board})
	add_executable(${target} bench.cpp startup.cpp)
	set_target_properties(${target} PROPERTIES SUFFIX ".elf"
		LINK_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/boards/${board}.ld;${CMAKE_CURRENT_SOURCE_DIR}/boards/common.ld")
	target_include_directories(${target} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../include)
	target_compile_definitions(${target} PRIVATE
		KYMO_BENCH_CORE_CLOCK_HZ=${clock_${board}}u
		KYMO_BENCH_RING_SLOTS=${KYMO_BENCH_RING_SLOTS})
	set(arch -mcpu=${cpu_${board}} -mthumb -mfloat-abi=soft)
	target_compile_options(${target} PRIVATE ${arch}
		-Os -g3 -Wall -Wextra -Werror
		-ffunction-sections -fdata-sections
		-fno-exceptions -fno-rtti -fno-threadsafe-statics)
	target_link_options(${target} PRIVATE ${arch}
		-nostdlib -Wl,--gc-sections -Wl,--build-id
		-L${CMAKE_CURRENT_SOURCE_DIR}/boards
		-T${CMAKE_CURRENT_SOURCE_DIR}/boards/${board}.ld
		-Wl,-Map=${target}.map)
	target_link_libraries(${target} PRIVATE gcc)
endforeach()
