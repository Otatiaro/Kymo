cmake_minimum_required(VERSION 3.20)

# The Zephyr adapter, on a test application (application.cpp) run under QEMU
# on the Cortex-M3 (mps2/an385) and the Cortex-M33 (mps2/an521/cpu0),
# recorded by the host and checked (trace/trace.py):
#   west build -b mps2/an385 -d build/zephyr-an385 target/zephyr/tests
#   python target/zephyr/tests/trace/trace.py --qemu <qemu-system-arm> \
#       --machine mps2-an385 --image build/zephyr-an385/zephyr/zephyr.elf \
#       --host host/target/release/kymo
set(ZEPHYR_EXTRA_MODULES ${CMAKE_CURRENT_SOURCE_DIR}/..)
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(kymo_zephyr_tests)

target_sources(app PRIVATE application.cpp)
target_include_directories(app PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../../tests/qemu)
target_compile_options(app PRIVATE -Wall -Wextra -Werror)

# The adapter's unit takes its tracer from here.
target_include_directories(kymo_zephyr PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})
target_compile_options(kymo_zephyr PRIVATE -Wall -Wextra -Werror)
