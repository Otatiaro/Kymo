# Toolchain file for the Arm GNU toolchain (arm-none-eabi-g++ on the PATH, or
# ARM_GCC_ROOT pointing at its install directory).
#
# The linker scripts INCLUDE a shared part. On Windows, ld 2.47 rejects any
# INCLUDE as "appears multiple times" (binutils PR 24576, fixed after 2.47):
# use Otatiaro/arm-none-eabi-toolchain 16.2.0-2 or later there.

set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_SYSTEM_PROCESSOR arm)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)

if(DEFINED ENV{ARM_GCC_ROOT})
	set(prefix "$ENV{ARM_GCC_ROOT}/bin/")
endif()

if(CMAKE_HOST_WIN32)
	set(suffix ".exe")
endif()

set(CMAKE_C_COMPILER "${prefix}arm-none-eabi-gcc${suffix}")
set(CMAKE_CXX_COMPILER "${prefix}arm-none-eabi-g++${suffix}")
set(CMAKE_ASM_COMPILER "${prefix}arm-none-eabi-gcc${suffix}")
