# Toolchain file for Clang on bare-metal Arm, with the Arm GNU toolchain's
# C and C++ headers and binutils (arm-none-eabi-gcc on the PATH, or
# ARM_GCC_ROOT pointing at its install directory).
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

if(DEFINED ENV{LLVM_ROOT})
	set(llvm_prefix "$ENV{LLVM_ROOT}/bin/")
endif()
if(CMAKE_HOST_WIN32)
	set(suffix ".exe")
endif()

set(CMAKE_C_COMPILER "${llvm_prefix}clang${suffix}")
set(CMAKE_CXX_COMPILER "${llvm_prefix}clang++${suffix}")
set(CMAKE_ASM_COMPILER "${llvm_prefix}clang${suffix}")
set(CMAKE_AR "${llvm_prefix}llvm-ar${suffix}" CACHE FILEPATH "")
set(CMAKE_RANLIB "${llvm_prefix}llvm-ranlib${suffix}" CACHE FILEPATH "")
set(CMAKE_C_COMPILER_TARGET arm-none-eabi)
set(CMAKE_CXX_COMPILER_TARGET arm-none-eabi)
set(CMAKE_ASM_COMPILER_TARGET arm-none-eabi)

# Clang reads no multilib of the GNU toolchain: kymo_clang_headers(<cpu> <out>)
# gives the C and C++ header directories GCC searches for that CPU, its own
# builtin headers left to Clang's. ld.lld links.
set(CMAKE_EXE_LINKER_FLAGS_INIT "-fuse-ld=lld")

function(kymo_clang_headers cpu out)
	file(TOUCH "${CMAKE_BINARY_DIR}/empty.cpp")
	execute_process(
		COMMAND "${KYMO_ARM_GCC}" -mcpu=${cpu} -mthumb -x c++ -E -v "${CMAKE_BINARY_DIR}/empty.cpp"
		OUTPUT_QUIET ERROR_VARIABLE listing COMMAND_ERROR_IS_FATAL ANY)
	string(REGEX MATCH "#include <...> search starts here:\n(.*)\nEnd of search list" _ "${listing}")
	string(REPLACE "\n" ";" directories "${CMAKE_MATCH_1}")
	set(flags -nostdlibinc)
	foreach(directory IN LISTS directories)
		string(STRIP "${directory}" directory)
		file(REAL_PATH "${directory}" directory)
		if(NOT directory MATCHES "/lib/gcc/")
			# One word each: CMake drops a repeated option, a lone -isystem included.
			list(APPEND flags "-isystem${directory}")
		endif()
	endforeach()
	set(${out} ${flags} PARENT_SCOPE)
endfunction()

# The tests' binutils: the GNU ones, which read Clang's objects too.
set(CMAKE_OBJCOPY "${gcc_bin}/arm-none-eabi-objcopy${suffix}" CACHE FILEPATH "")
set(CMAKE_NM "${gcc_bin}/arm-none-eabi-nm${suffix}" CACHE FILEPATH "")
set(CMAKE_SIZE "${gcc_bin}/arm-none-eabi-size${suffix}" CACHE FILEPATH "")
