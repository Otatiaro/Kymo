/**
 * @file    fault.cpp
 * @brief   kymo_freertos_fault(): a fault three calls deep, recorded by the
 *          application's fault handler, its event read back from the ring.
 *
 * In order, in a task, each fault stepped over by the handler:
 *
 * - in the task: the frame on the process stack, whose bounds the adapter
 *   does not know, searched 64 words up;
 * - in the device interrupt's handler: the frame on the main stack, searched
 *   up to the vector table's initial stack pointer, a decoy return address
 *   above it;
 * - on a core with an FPU, with a floating-point frame, its registers
 *   holding a decoy return address that the search skips.
 *
 * Each event is checked against what the handler was given (the frame, its
 * address, EXC_RETURN, the fault registers) and holds the three return
 * addresses each level records as it is entered.
 */
#include "platform.hpp"
#include "qemu.hpp"

#include <FreeRTOS.h>
#include <task.h>
#include <kymo/freertos/trace.h>

#include <fault_ring.hpp>
#include <kymo/fault.hpp>
#include <kymo_freertos_trace.hpp>

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>

namespace
{

constexpr std::uintptr_t cfsr = 0xE000ED28;
constexpr std::uintptr_t hfsr = 0xE000ED2C;
constexpr std::uintptr_t mmfar = 0xE000ED34;
constexpr std::uintptr_t bfar = 0xE000ED38;
constexpr std::uintptr_t shcsr = 0xE000ED24;
constexpr std::uint32_t fault_enables = 7U << 16;   // SHCSR: MemManage, BusFault, UsageFault
constexpr std::uint32_t precise_bus_fault = (1U << 9) | (1U << 15);   // CFSR: PRECISERR, BFARVALID
constexpr std::uint32_t unmapped = 0xF0000000U;
constexpr std::uint32_t process_stack = 1U << 2;   // EXC_RETURN.SPSEL
constexpr std::uint32_t no_fp_frame = 1U << 4;     // EXC_RETURN.FType
constexpr std::uint32_t code_end = 0x20000000U;
constexpr std::size_t frame_words = 8;

/// What the handler was given, and where in the ring it recorded the fault.
struct handled
{
	std::uint32_t index;
	std::uint32_t frame_address;
	std::array<std::uint32_t, frame_words> frame;
	std::uint32_t exc_return;
	std::array<std::uint32_t, 4> status;   // CFSR, HFSR, MMFAR, BFAR
};

// NOLINTBEGIN(cppcoreguidelines-avoid-non-const-global-variables): shared with the handlers
handled last{};
std::uint32_t volatile faults = 0;
/// Each level's return address, as it is entered: [0] in level_two, [1] in level_one, [2] in its caller.
std::array<std::uint32_t volatile, 3> called_from{};
/// Whether level_three loads the floating-point registers with the decoy first.
bool volatile with_decoy = false;
StaticTask_t task_tcb;
std::array<StackType_t, configMINIMAL_STACK_SIZE> task_stack;

/// The main stack once the scheduler runs, and above its top, words a search past it would keep.
struct
{
	alignas(8) std::array<std::uint32_t, 512> stack;
	std::array<std::uint32_t, kymo::fault_search_words> above;
} main_stack;
// NOLINTEND(cppcoreguidelines-avoid-non-const-global-variables)

std::uint32_t address_of(void const* pointer)
{
	return static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(pointer));   // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast): an address, compared
}

[[noreturn]] void fail(char const* why)
{
	qemu::write("*** ");
	qemu::write(why);
	qemu::write(" ***\n");
	qemu::finish(false);
}

void expect(bool condition, char const* what)
{
	if (!condition)
		fail(what);
}

/** @brief Odd and in the Code region, as a return address: what the decoy must look like to be skipped for its place alone. */
[[gnu::noinline]] void decoy()
{
	asm volatile("" ::: "memory");
}

/** @brief A 4-byte load nothing answers: a precise BusFault, a return address in lr, none pushed. */
[[gnu::noinline]] void level_three()
{
	called_from[0] = address_of(__builtin_return_address(0));
#ifdef __ARM_FP
	if (with_decoy)
	{
		std::uint32_t const fake = address_of(reinterpret_cast<void const*>(&decoy));   // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast): a function's address, as data
		asm volatile(
				"vmov s0, %0 \n\t" "vmov s1, %0 \n\t" "vmov s2, %0 \n\t" "vmov s3, %0 \n\t"
				"vmov s4, %0 \n\t" "vmov s5, %0 \n\t" "vmov s6, %0 \n\t" "vmov s7, %0 \n\t"
				"vmov s8, %0 \n\t" "vmov s9, %0 \n\t" "vmov s10, %0 \n\t" "vmov s11, %0 \n\t"
				"vmov s12, %0 \n\t" "vmov s13, %0 \n\t" "vmov s14, %0 \n\t" "vmov s15, %0"
				:: "r"(fake)
				: "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "s12", "s13", "s14", "s15");
	}
#endif
	std::uint32_t value = 0;
	asm volatile("ldr.w %0, [%1]" : "=r"(value) : "r"(unmapped) : "memory");
}

[[gnu::noinline]] void level_two()
{
	called_from[1] = address_of(__builtin_return_address(0));
	level_three();
	asm volatile("" ::: "memory");   // after the call: not a tail call
}

[[gnu::noinline]] void level_one()
{
	called_from[2] = address_of(__builtin_return_address(0));
	level_two();
	asm volatile("" ::: "memory");
}

void on_interrupt()
{
	level_one();
	asm volatile("" ::: "memory");
}

/** @brief The event of the fault just stepped over, checked against what the handler was given. */
kymo::fault last_fault(std::uint32_t faults_before, char const* name)
{
	expect(faults == faults_before + 1, name);
	kymo::fault event{};
	expect(read_fault(trace_ring, last.index, event), "no kymo::fault where the handler started writing");
	std::array<std::uint32_t, frame_words> const recorded{event.r0, event.r1, event.r2, event.r3, event.r12, event.lr, event.pc, event.xpsr};
	expect(recorded == last.frame, "the frame recorded is not the frame the handler was given");
	expect(event.exc_return == last.exc_return, "EXC_RETURN recorded is not the handler's");
	expect(event.sp == last.frame_address, "sp is not the frame's address");
	std::array<std::uint32_t, 4> const status{event.cfsr, event.hfsr, event.mmfar, event.bfar};
	expect(status == last.status, "the fault registers recorded are not the handler's");
	expect((event.cfsr & precise_bus_fault) == precise_bus_fault && event.bfar == unmapped, "not the precise BusFault of the load");
	return event;
}

/** @brief The three return addresses among the event's lr and kept words, nearest first; no decoy among them. */
void expect_levels(kymo::fault const& event)
{
	std::array<std::uint32_t, kymo::fault_return_count + 1> words{};
	words[0] = event.lr;
	for (std::size_t i = 0; i < event.returns.size(); ++i)
		words[i + 1] = event.returns[i];

	for (auto const word : event.returns)
		expect(word == 0 || ((word & 1U) != 0 && word < code_end), "a kept word is not odd in the Code region");

	std::size_t found = 0;
	for (auto const word : words)
	{
		expect(word != address_of(reinterpret_cast<void const*>(&decoy)), "a decoy was kept");   // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast): a function's address, as data
		if (found < called_from.size() && word == called_from[found])
			++found;
	}
	expect(found == called_from.size(), "the three return addresses are not in the event, nearest first");
}

void report(char const* name)
{
	qemu::write("  ok: ");
	qemu::write(name);
	qemu::write("\n");
}

void in_a_task()
{
	std::uint32_t const before = faults;
	level_one();
	auto const event = last_fault(before, "in a task: no fault");
	expect((event.exc_return & process_stack) != 0, "in a task: the frame is not on the process stack");
	expect(event.stack_base == 0 && event.stack_top == 0, "in a task: the bounds are not unknown");
	expect(event.sp >= address_of(task_stack.data()) && event.sp < address_of(task_stack.data()) + sizeof(task_stack), "in a task: the frame is outside the task's stack");
	expect_levels(event);
	report("in a task");
}

void in_an_interrupt()
{
	std::uint32_t const before = faults;
	platform::pend();
	asm volatile("dsb\n\tisb" ::: "memory");
	auto const event = last_fault(before, "in an interrupt: no fault");
	expect((event.exc_return & process_stack) == 0, "in an interrupt: the frame is not on the main stack");
	expect(event.stack_base == 0 && event.stack_top == address_of(main_stack.above.data()), "in an interrupt: the bounds are not the main stack's");
	expect_levels(event);
	report("in an interrupt");
}

#ifdef __ARM_FP
void with_a_floating_point_frame()
{
	// The floating-point registers stacked on entry, not left to the handler:
	// the decoy is in the frame.
	constexpr std::uintptr_t fpccr = 0xE000EF34;
	constexpr std::uint32_t lspen = 1U << 30;
	platform::scs(fpccr) &= ~lspen;
	with_decoy = true;
	std::uint32_t const before = faults;
	level_one();
	with_decoy = false;
	platform::scs(fpccr) |= lspen;
	auto const event = last_fault(before, "with a floating-point frame: no fault");
	expect((event.exc_return & no_fp_frame) == 0, "with a floating-point frame: EXC_RETURN says none was stacked");
	expect_levels(event);
	report("with a floating-point frame");
}
#endif

void faulting_task(void* /*unused*/)
{
	in_a_task();
	in_an_interrupt();
#ifdef __ARM_FP
	with_a_floating_point_frame();
#endif
	qemu::write("every fault carried its call levels\n");
	qemu::finish(true);
}

} // namespace

extern "C"
{
void application_assert(char const* file, int line)
{
	qemu::write("assertion failed: ");
	qemu::write(file);
	qemu::write(":");
	qemu::write_unsigned(static_cast<std::uint32_t>(line));
	qemu::write("\n");
	qemu::finish(false);
}

/**
 * @brief The application's fault handler, past the entry stub: records the
 *        fault, then steps over the load and clears the status.
 */
[[gnu::used]] void application_fault(std::uint32_t* frame, std::uint32_t exc_return)
{
	last.index = trace_ring.write_index.load(std::memory_order_relaxed);
	kymo_freertos_fault(frame, exc_return);

	last.frame_address = address_of(frame);
	// NOLINTBEGIN(cppcoreguidelines-pro-bounds-pointer-arithmetic): the frame the core stacked
	for (std::size_t i = 0; i < frame_words; ++i)
		last.frame.at(i) = frame[i];
	last.exc_return = exc_return;
	last.status = {platform::scs(cfsr), platform::scs(hfsr), platform::scs(mmfar), platform::scs(bfar)};
	faults = faults + 1;
	frame[6] += 4;   // pc, past the ldr.w
	// NOLINTEND(cppcoreguidelines-pro-bounds-pointer-arithmetic)
	platform::scs(cfsr) = last.status[0];   // write one to clear
}

/** @brief The entry stub: the frame from the stack EXC_RETURN names, and EXC_RETURN itself. */
[[gnu::naked]] void fault_entry()
{
	asm volatile(
			"tst   lr, #4 \n\t"
			"ite   eq \n\t"
			"mrseq r0, msp \n\t"
			"mrsne r0, psp \n\t"
			"mov   r1, lr \n\t"
			"b     application_fault");
}
}

void test_main()
{
	platform::prepare(&platform::port_systick, &on_interrupt);
	// HardFault, MemManage, BusFault and UsageFault, in the table prepare() installed.
	auto* const vectors = reinterpret_cast<platform::vector*>(platform::scs(0xE000ED08));   // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast,performance-no-int-to-ptr): VTOR
	for (std::size_t slot = 3; slot <= 6; ++slot)
		vectors[slot] = platform::vector_of(&fault_entry);   // NOLINT(cppcoreguidelines-pro-bounds-pointer-arithmetic): the table's own slots
	// The main stack the port starts the scheduler on, from the table's first
	// word, with the decoy above it.
	main_stack.above.fill(address_of(reinterpret_cast<void const*>(&decoy)));   // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast): a function's address, as data
	*vectors = reinterpret_cast<platform::vector>(main_stack.above.data());   // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast): the initial stack pointer
	platform::scs(shcsr) |= fault_enables;
	asm volatile("dsb\n\tisb" ::: "memory");

	xTaskCreateStatic(faulting_task, "faulting", task_stack.size(), nullptr, 1, task_stack.data(), &task_tcb);
	vTaskStartScheduler();
	application_assert(__FILE__, __LINE__);   // the scheduler does not return
}
