"""The oracle's handling of the SWO on packets no clean capture holds: a
handler left open at the end of a stretch, an exit of one entered before the
stretch, an exit that is not the innermost one, a return to thread mode with
handlers open, and an adapter's span outside the SWO's span that would give
its costs."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import oracle  # noqa: E402


class Nest(unittest.TestCase):
    def test_a_handler_open_at_the_end_of_a_stretch_ends_there_lost(self):
        lines, contradictions = oracle.nest([(0, 100)], [(10, 61, "entered")])
        self.assertEqual(lines, [(61, (10, False), (100, True))])
        self.assertEqual(contradictions, [])

    def test_an_exit_of_a_handler_entered_before_the_stretch_starts_it_lost(self):
        lines, contradictions = oracle.nest([(0, 100)], [(10, 61, "exited")])
        self.assertEqual(lines, [(61, (0, True), (10, False))])
        self.assertEqual(contradictions, [])

    def test_an_exit_that_is_not_the_innermost_contradicts(self):
        packets = [(10, 15, "entered"), (20, 61, "entered"), (30, 15, "exited")]
        lines, contradictions = oracle.nest([(0, 100)], packets)
        self.assertEqual(contradictions, [30])
        self.assertNotIn((61, (20, False), (30, False)), lines)


class TakeSwo(unittest.TestCase):
    @staticmethod
    def found(interrupts):
        return {"interrupts": interrupts, "contradictions": [], "traced": {61: ("IRQ45 (isr)", "isr")}}

    ITEMS = [{"kind": "event", "cycles": 5}, {"kind": "event", "cycles": 15}, {"kind": "event", "cycles": 35}]

    def test_a_return_to_thread_mode_with_a_handler_open_contradicts_at_its_element(self):
        found = self.found([])
        decoded = "swo 0 100\nexception 10 61 entered\nexception 20 0 returned\n"
        oracle.take_swo(found, self.ITEMS, decoded, {}, {})
        self.assertEqual(found["contradictions"], [1])

    def test_an_adapter_span_the_swo_span_does_not_hold_gives_no_cost(self):
        found = self.found([("IRQ45 (isr)", 0, (2, 25, False), (2, 30, False), False, "isr")])
        decoded = "swo 0 100\nexception 10 61 entered\nexception 20 61 exited\n"
        oracle.take_swo(found, self.ITEMS, decoded, {}, {})
        self.assertEqual(found["costs"], [])

    def test_an_adapter_span_the_swo_span_holds_gives_its_costs(self):
        found = self.found([("IRQ45 (isr)", 0, (2, 25, False), (2, 30, False), False, "isr")])
        decoded = "swo 0 100\nexception 20 61 entered\nexception 40 61 exited\n"
        oracle.take_swo(found, self.ITEMS, decoded, {}, {})
        self.assertEqual(found["costs"], [("IRQ45 (isr)", (1, 20, False), (2, 25, False), (2, 30, False),
                                           (2, 40, False))])


if __name__ == "__main__":
    unittest.main()
