/**
 * @file    application.cpp
 * @brief   An event of the application's own, in an image that runs OpSy
 *          without Kymo's hooks: the host decodes the image, and warns that
 *          OpSy's events are missing.
 */
#include <opsy_trace.hpp>

#include <cstdint>

/// An event of the application, family application.
struct application_started
{
	std::uint32_t code;
};

/// Never called, kept by the link (-Wl,--undefined): it holds the event's
/// type id and layout in the image.
extern "C" void start_application();

extern "C" void start_application()
{
	trace::emit(application_started{1});
}
