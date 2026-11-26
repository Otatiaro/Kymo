# The Zephyr adapter, a Zephyr module: the application adds this directory
# to ZEPHYR_EXTRA_MODULES and sets CONFIG_KYMO=y (Kconfig).
if(CONFIG_KYMO)
	zephyr_include_directories(include ${CMAKE_CURRENT_LIST_DIR}/../include)
	zephyr_library_named(kymo_zephyr)
	zephyr_library_sources(kymo_zephyr.cpp)
	zephyr_linker_sources(SECTIONS kymo.ld)
endif()
