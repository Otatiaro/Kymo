# Checks the type ids of an image built from first.cpp and second.cpp.
#   cmake -DELF=<image> -DREADELF=<readelf> -DNM=<nm> -P check.cmake
#
# - .kymo.ids is in the image and is not loaded on the target;
# - it holds one byte per event type, a type used in two units included;
# - each type has its symbol, and the ids are distinct.

foreach(var ELF READELF NM)
	if(NOT DEFINED ${var})
		message(FATAL_ERROR "${var} is not set")
	endif()
endforeach()

execute_process(COMMAND ${READELF} -S --wide ${ELF} OUTPUT_VARIABLE sections COMMAND_ERROR_IS_FATAL ANY)
string(REGEX MATCH "\\.kymo\\.ids +PROGBITS +([0-9a-f]+) +[0-9a-f]+ +([0-9a-f]+) +[0-9a-f]+ +([A-Z]*) " line "${sections}")
if(NOT line)
	message(FATAL_ERROR "no .kymo.ids section in ${ELF}")
endif()
set(flags "${CMAKE_MATCH_3}")
math(EXPR size "0x${CMAKE_MATCH_2}")
if(flags MATCHES "A")
	message(FATAL_ERROR ".kymo.ids is loaded on the target (flags ${flags})")
endif()

execute_process(COMMAND ${NM} ${ELF} OUTPUT_VARIABLE symbols COMMAND_ERROR_IS_FATAL ANY)
set(ids "")
foreach(event a b c)
	string(REGEX MATCH "([0-9a-f]+) [A-Za-z] _ZN4kymo6anchorI7event_${event}E4nameE\\.kymo_id" line "${symbols}")
	if(NOT line)
		message(FATAL_ERROR "no id symbol for event_${event}")
	endif()
	math(EXPR id "0x${CMAKE_MATCH_1}")
	if(id IN_LIST ids)
		message(FATAL_ERROR "event_${event} shares id ${id} with another type")
	endif()
	if(id GREATER_EQUAL size)
		message(FATAL_ERROR "event_${event} has id ${id}, outside .kymo.ids (${size} bytes)")
	endif()
	list(APPEND ids ${id})
endforeach()

if(NOT size EQUAL 3)
	message(FATAL_ERROR ".kymo.ids holds ${size} bytes for 3 event types")
endif()

message(STATUS "type ids: event_a, event_b, event_c = ${ids}")
