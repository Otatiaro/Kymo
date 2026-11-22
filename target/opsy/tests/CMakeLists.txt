cmake_minimum_required(VERSION 3.25)

# The OpSy adapter, on OpSy's own QEMU suite: the suite's images built with
# Kymo's opsy_hooks.hpp on their include path, so that every hook OpSy calls
# while the suite runs emits its event. One core per build tree, like OpSy's:
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
	${CMAKE_CURRENT_SOURCE_DIR}/../include)

foreach(image opsy_qemu_tests opsy_qemu_start_check opsy_qemu_before_start)
	target_include_directories(${image} PRIVATE ${kymo_include})
endforeach()

# The fault image, traced: the adapter's events and the fault recording OpSy's
# test_faults.cpp checks, ahead of OpSy's own fault hooks.
target_include_directories(opsy_qemu_faults BEFORE PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/faults ${kymo_include})

# The bench, traced: its markers and the adapter's events together, ahead of
# OpSy's own bench hooks. bench/overhead.py then holds what tracing adds to
# each path against its record.
target_include_directories(opsy_qemu_bench BEFORE PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/bench ${kymo_include})

# The adapter's events read back from the ring, as a case of OpSy's suite.
target_sources(opsy_qemu_tests PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/ring_check.cpp)
