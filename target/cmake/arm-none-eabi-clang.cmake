# Toolchain file for Clang on bare-metal Arm, with the Arm GNU toolchain's
# C and C++ headers and binutils (arm-none-eabi-gcc on the PATH, or
# ARM_GCC_ROOT pointing at its install directory); ld.lld links.
#
# Clang and ld.lld are taken from the PATH, or from LLVM_ROOT/bin.

set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_SYSTEM_PROCESSOR arm)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)

if(DEFINED ENV{ARM_GCC_ROOT})
	set(gcc_hint "$ENV{ARM_GCC_ROOT}/bin")
endif()
find_program(KYMO_ARM_GCC arm-none-eabi-gcc HINTS ${gcc_hint} REQUIRED)
get_filename_component(gcc_bin "${KYMO_ARM_GCC}" DIRECTORY)
get_filename_component(gcc_root "${gcc_bin}" DIRECTORY)

if(DEFINED ENV{LLVM_ROOT})
	set(llvm_prefix "$ENV{LLVM_ROOT}/bin/")
endif()
if(CMAKE_HOST_WIN32)
	set(suffix ".exe")
endif()

set(CMAKE_C_COMPILER "${llvm_prefix}clang${suffix}")
set(CMAKE_CXX_COMPILER "${llvm_prefix}clang++${suffix}")
set(CMAKE_ASM_COMPILER "${llvm_prefix}clang${suffix}")
set(CMAKE_C_COMPILER_TARGET arm-none-eabi)
set(CMAKE_CXX_COMPILER_TARGET arm-none-eabi)
set(CMAKE_ASM_COMPILER_TARGET arm-none-eabi)

# The GNU toolchain's headers, its multilib for the -mcpu given, and ld.lld.
set(CMAKE_C_FLAGS_INIT "--gcc-toolchain=${gcc_root}")
set(CMAKE_CXX_FLAGS_INIT "--gcc-toolchain=${gcc_root}")
set(CMAKE_EXE_LINKER_FLAGS_INIT "--gcc-toolchain=${gcc_root} -fuse-ld=lld")

# The tests' binutils: the GNU ones, which read Clang's objects too.
set(CMAKE_OBJCOPY "${gcc_bin}/arm-none-eabi-objcopy${suffix}" CACHE FILEPATH "")
set(CMAKE_NM "${gcc_bin}/arm-none-eabi-nm${suffix}" CACHE FILEPATH "")
set(CMAKE_SIZE "${gcc_bin}/arm-none-eabi-size${suffix}" CACHE FILEPATH "")
