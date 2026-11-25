/**
 * @file    levels.cpp
 * @brief   The call levels a kymo::fault carries: a fault three calls deep,
 *          its event read back from the ring, the words the adapter kept
 *          holding each call's return address, nearest first.
 *
 * In order, each fault stepped over by the application's handler:
 *
 * - in a task: the frame on the process stack, searched up to the top of
 *   the task's stack;
 * - in an interrupt handler: the frame on the main stack, searched up to the
 *   vector table's initial stack pointer;
 * - on a core with an FPU, in a task with a floating-point frame, its
 *   registers holding a decoy return address that the search skips;
 * - in a task running on a stack that is not its own: recorded, not searched.
 *
 * Each level records its own return address (__builtin_return_address) as
 * it is entered: what the event's words are checked against.
 */

#include "qemu_test.hpp"
#include "semihosting.hpp"
#include "thumb.hpp"

#include <fault_ring.hpp>
#include <opsy.hpp>
#include <opsy_trace.hpp>

#include <array>
#include <cstddef>
#include <cstdint>

extern "C"
{
extern std::uint32_t _estack;
void Reset_Handler();
}

namespace
{

using handler_t = void (*)();

/** @brief The shape SvdGenerator generates, enough of it for with(). */
enum class exception : std::uint8_t { nmi = 2, hard_fault = 3, mem_manage = 4, bus_fault = 5, usage_fault = 6, svcall = 11, pendsv = 14, systick = 15 };

[[noreturn]] void fail(const char* why)
{
	qemu::write("*** ");
	qemu::write(why);
	qemu::write(" ***\nRESULT: FAIL\n");
	qemu::exit(1);
}

void expect(bool condition, const char* what)
{
	if (!condition)
		fail(what);
}

[[noreturn]] void unexpected_exception()
{
	fail("an exception the table does not expect was taken");
}

/** @brief The initial stack pointer, then the handlers by slot; in RAM, for the interrupt installed at run time. */
struct alignas(512) vector_table
{
	using exception_type = exception;

	constexpr vector_table(std::uint32_t* stack, handler_t reset) : initial_sp(stack)
	{
		for (auto& handler : handlers)
			handler = &unexpected_exception;
		handlers[0] = reset;
	}

	template<exception Exception, handler_t Handler>
	[[nodiscard]] constexpr auto with() const -> vector_table
	{
		auto copy = *this;
		copy.handlers[static_cast<std::size_t>(Exception) - 1] = Handler;
		return copy;
	}

	[[nodiscard]] constexpr auto with(auto&& install) const { return install(*this); }

	std::uint32_t* initial_sp;
	std::array<handler_t, 15 + 64> handlers{};
};

constexpr std::uint32_t vtor = 0xE000ED08U;
constexpr std::uint32_t fpccr = 0xE000EF34U;
constexpr std::uint32_t fpccr_lspen = 1U << 30;
constexpr std::uint32_t unmapped = 0xF0000000U;
constexpr std::uint32_t exc_return_psp = 1U << 2;

opsy::task<1024> g_task;
opsy::idle_task<256> g_idle;
unsigned g_irq = qemu_test::no_irq;
volatile std::uint32_t g_faults = 0;

/// Each level's return address, as it is entered: [0] in level_two, [1] in level_one, [2] in its caller.
std::array<volatile std::uint32_t, 3> g_called_from{};
/// Whether level_three loads the floating-point registers with the decoy first.
volatile bool g_decoy = false;
/// Where the stack that is not the task's runs from.
alignas(8) std::array<std::uint32_t, 128> g_other_stack{};

/** @brief Odd and in the Code region, as a return address: what the decoy must look like to be skipped for its place alone. */
[[gnu::noinline]] void decoy()
{
	asm volatile("" ::: "memory");
}

std::uint32_t return_address(void* address)
{
	return static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(address));
}

/** @brief A load nothing answers: a precise BusFault, a return address in lr, none pushed. */
[[gnu::noinline]] void level_three()
{
	g_called_from[0] = return_address(__builtin_return_address(0));
#ifdef __ARM_FP
	if (g_decoy)
	{
		std::uint32_t const fake = reinterpret_cast<std::uintptr_t>(&decoy);
		asm volatile(
				"vmov s0, %0 \n\t" "vmov s1, %0 \n\t" "vmov s2, %0 \n\t" "vmov s3, %0 \n\t"
				"vmov s4, %0 \n\t" "vmov s5, %0 \n\t" "vmov s6, %0 \n\t" "vmov s7, %0 \n\t"
				"vmov s8, %0 \n\t" "vmov s9, %0 \n\t" "vmov s10, %0 \n\t" "vmov s11, %0 \n\t"
				"vmov s12, %0 \n\t" "vmov s13, %0 \n\t" "vmov s14, %0 \n\t" "vmov s15, %0"
				:: "r"(fake)
				: "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "s12", "s13", "s14", "s15");
	}
#endif
	(void) *reinterpret_cast<volatile std::uint32_t*>(unmapped);
}

[[gnu::noinline]] void level_two()
{
	g_called_from[1] = return_address(__builtin_return_address(0));
	level_three();
	asm volatile("" ::: "memory"); // after the call: not a tail call
}

[[gnu::noinline]] void level_one()
{
	g_called_from[2] = return_address(__builtin_return_address(0));
	level_two();
	asm volatile("" ::: "memory");
}

void on_interrupt()
{
	level_one();
	asm volatile("" ::: "memory");
}

/** @brief Calls level_one with the stack pointer in g_other_stack, PSPLIM lifted for it on ARMv8-M. */
[[gnu::noinline]] void on_another_stack()
{
	std::uint32_t* const top = g_other_stack.data() + g_other_stack.size();
	asm volatile(
#ifdef __ARM_ARCH_8M_MAIN__
			"mrs r5, psplim \n\t"
			"movs r6, #0 \n\t"
			"msr psplim, r6 \n\t"
#endif
			"mov r4, sp \n\t"
			"mov sp, %[top] \n\t"
			"blx %[call] \n\t"
			"mov sp, r4 \n\t"
#ifdef __ARM_ARCH_8M_MAIN__
			"msr psplim, r5 \n\t"
#endif
			:: [top] "r"(top), [call] "r"(&level_one)
			: "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r12", "lr", "memory", "cc"
#ifdef __ARM_FP
			, "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "s12", "s13", "s14", "s15"
#endif
			);
}

/** @brief The application's handler: steps over the faulting load, clears the status, returns. */
void on_fault(const opsy::fault_status& /*status*/, opsy::exception_frame& frame, std::uint32_t /*exc_return*/)
{
	g_faults = g_faults + 1;
	frame.pc += qemu_test::thumb_instruction_length(frame.pc);
	opsy::cortex_m::clear_fault_status();
}

constinit vector_table g_table = vector_table(&_estack, Reset_Handler)
	.with(opsy::scheduler::isrs)
	.with(opsy::fault_isrs<on_fault>);

/** @brief The event of the fault just stepped over. */
kymo::fault last_fault(std::uint32_t faults_before, const char* name)
{
	expect(g_faults == faults_before + 1, name);
	kymo::fault event{};
	expect(read_fault(trace_ring, opsy::hooks::fault_index, event), "no kymo::fault where the hook started writing");
	return event;
}

/** @brief The three return addresses among the event's lr and kept words, nearest first; none of them the decoy. */
void expect_levels(const kymo::fault& event)
{
	std::array<std::uint32_t, 9> words{};
	words[0] = event.lr;
	for (std::size_t i = 0; i < event.returns.size(); ++i)
		words[i + 1] = event.returns[i];

	for (auto const word : event.returns)
		expect(word == 0 || ((word & 1U) != 0 && word < 0x20000000U), "a kept word is not odd in the Code region");

	std::size_t found = 0;
	for (auto const word : words)
	{
		expect(word != reinterpret_cast<std::uintptr_t>(&decoy), "the decoy in the floating-point frame was kept");
		if (found < g_called_from.size() && word == g_called_from[found])
			++found;
	}
	expect(found == g_called_from.size(), "the three return addresses are not in the event, nearest first");
}

void report(const char* name)
{
	qemu::write("  ok: ");
	qemu::write(name);
	qemu::write("\n");
}

std::uint32_t address_of(const void* pointer)
{
	return static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(pointer));
}

void in_a_task()
{
	std::uint32_t const before = g_faults;
	level_one();
	auto const event = last_fault(before, "in a task: no fault");
	std::uint32_t const base = address_of(g_task.stack_base());
	expect((event.exc_return & exc_return_psp) != 0, "in a task: the frame is not on the process stack");
	expect(event.stack_base == base && event.stack_top == base + g_task.stack_size(), "in a task: the bounds are not the task's stack");
	expect(event.sp >= event.stack_base && event.sp < event.stack_top, "in a task: the frame is outside the task's stack");
	expect_levels(event);
	report("in a task");
}

void in_an_interrupt()
{
	std::uint32_t const before = g_faults;
	qemu_test::raise(g_irq);
	auto const event = last_fault(before, "in an interrupt: no fault");
	expect((event.exc_return & exc_return_psp) == 0, "in an interrupt: the frame is not on the main stack");
	expect(event.stack_base == 0 && event.stack_top == address_of(&_estack), "in an interrupt: the bounds are not the main stack's");
	expect_levels(event);
	report("in an interrupt");
}

#ifdef __ARM_FP
void with_a_floating_point_frame()
{
	// The floating-point registers stacked on entry, not left to the handler:
	// the decoy is in the frame.
	*reinterpret_cast<volatile std::uint32_t*>(fpccr) &= ~fpccr_lspen;
	g_decoy = true;
	std::uint32_t const before = g_faults;
	level_one();
	g_decoy = false;
	auto const event = last_fault(before, "with a floating-point frame: no fault");
	expect((event.exc_return & (1U << 4)) == 0, "with a floating-point frame: EXC_RETURN says none was stacked");
	expect_levels(event);
	report("with a floating-point frame");
}
#endif

void on_a_stack_not_its_own()
{
	std::uint32_t const before = g_faults;
	on_another_stack();
	auto const event = last_fault(before, "on a stack not its own: no fault");
	std::uint32_t const base = address_of(g_task.stack_base());
	expect(event.stack_base == base && event.stack_top == base + g_task.stack_size(), "on a stack not its own: the bounds are not the task's stack");
	expect(event.sp >= address_of(g_other_stack.data()) && event.sp < address_of(g_other_stack.data() + g_other_stack.size()), "on a stack not its own: the frame is not where the task ran");
	for (auto const word : event.returns)
		expect(word == 0, "on a stack not its own: the stack was searched");
	report("on a stack not its own");
}

void run_cases()
{
	opsy::cortex_m::enable_configurable_faults();

	in_a_task();

	g_irq = qemu_test::find_usable_irq();
	expect(g_irq != qemu_test::no_irq, "no interrupt the NVIC accepts");
	opsy::cortex_m::set_priority(g_irq, opsy::scheduler::os_callable_priority);
	g_table.handlers[16 + g_irq - 1] = &on_interrupt;
	opsy::cortex_m::enable_interrupt(g_irq);
	in_an_interrupt();
	opsy::cortex_m::disable_interrupt(g_irq);

	on_a_stack_not_its_own();
#ifdef __ARM_FP
	with_a_floating_point_frame();
#endif

	qemu::write("every fault carried its call levels\nRESULT: PASS\n");
	qemu::exit(0);
}

} // namespace

int opsy_test_main()
{
	qemu::write("Kymo fault call levels\n\n");

	*reinterpret_cast<volatile std::uint32_t*>(vtor) = reinterpret_cast<std::uint32_t>(&g_table);
	asm volatile("dsb\n\tisb" ::: "memory");

	(void) g_task.start([] { run_cases(); });

	opsy::scheduler::start(g_idle);
}
