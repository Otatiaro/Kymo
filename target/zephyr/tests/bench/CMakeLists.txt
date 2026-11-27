cmake_minimum_required(VERSION 3.20)

# The cost bench of the Zephyr adapter (bench.cpp), built untraced and traced,
# and the difference held against overhead-<cpu>.json:
#   west build -b mps2/an385 -d build/bench-plain target/zephyr/tests/bench
#   west build -b mps2/an385 -d build/bench-traced target/zephyr/tests/bench -- -DEXTRA_CONF_FILE=traced.conf
#   python target/opsy/tests/bench/overhead.py --profile <OpSy>/tests/qemu/bench/profile.py \
#       --machine mps2-an385 --plain build/bench-plain/zephyr/zephyr.elf \
#       --traced build/bench-traced/zephyr/zephyr.elf \
#       --expected target/zephyr/tests/bench/overhead-m3.json
set(ZEPHYR_EXTRA_MODULES ${CMAKE_CURRENT_SOURCE_DIR}/../..)
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(kymo_zephyr_bench)

target_sources(app PRIVATE bench.cpp)
target_include_directories(app PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../../../tests/qemu)
target_compile_options(app PRIVATE -Wall -Wextra -Werror)

# On a board (nucleo_h563zi), the markers read the cycle counter and the image
# prints cycles over semihosting:
#   west build -b nucleo_h563zi -d build/bench-h563 target/zephyr/tests/bench
#   probe-rs run --chip STM32H563ZITx build/bench-h563/zephyr/zephyr.elf
if(NOT CONFIG_QEMU_TARGET)
	target_compile_definitions(app PRIVATE KYMO_BENCH_BOARD)
endif()

# The sleep scenario begins in the kernel's SysTick handler.
zephyr_link_libraries(-Wl,--wrap=sys_clock_isr)

# The adapter's unit takes the test application's tracer.
if(CONFIG_KYMO)
	target_include_directories(kymo_zephyr PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/..)
	target_compile_options(kymo_zephyr PRIVATE -Wall -Wextra -Werror)
endif()
