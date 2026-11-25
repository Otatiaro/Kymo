use super::*;

const LINES: [Line; 5] = [
    Line::Cpu,
    Line::Section("TASKS"),
    Line::Task(Some(0)),
    Line::Task(None),
    Line::Event(0),
];

#[test]
fn a_line_is_found_from_its_top_to_before_the_next() {
    let tops = tops(&LINES, 0.0);
    assert_eq!(
        tops,
        [
            0.0,
            CPU,
            CPU + ROW,
            CPU + ROW + TASK,
            CPU + ROW + 2.0 * TASK,
            CPU + 2.0 * ROW + 2.0 * TASK
        ]
    );
    const { assert!(CPU > TASK && TASK > ROW) };
    for (n, line) in LINES.iter().enumerate() {
        let (top, bottom) = (tops[n], tops[n] + line.height());
        assert_eq!(tops[n + 1], bottom);
        assert_eq!(at(&tops, top), Some(n));
        assert_eq!(at(&tops, bottom - 0.5), Some(n));
    }
    assert_eq!(at(&tops, -0.5), None);
    assert_eq!(at(&tops, tops[LINES.len()]), None);
    assert_eq!(at(&super::tops(&[], 100.0), 0.0), None);
}

/// Lines given more room than they take fill it, sections keeping their
/// height and the other lines stretched alike; given less, they keep theirs.
#[test]
fn lines_stretch_to_fill_their_room() {
    let natural = tops(&LINES, 0.0);
    let total = natural[LINES.len()];
    assert_eq!(tops(&LINES, total - 10.0), natural);
    assert_eq!(tops(&LINES, total), natural);
    let room = 2.0 * total;
    let stretched = tops(&LINES, room);
    assert!((stretched[LINES.len()] - room).abs() < 1e-3, "{stretched:?}");
    let scale = (room - ROW) / (total - ROW);
    for (n, line) in LINES.iter().enumerate() {
        let height = stretched[n + 1] - stretched[n];
        let wanted = match line {
            Line::Section(_) => ROW,
            _ => line.height() * scale,
        };
        assert!((height - wanted).abs() < 1e-3, "{n} {stretched:?}");
    }
    assert_eq!(tops(&[Line::Section("TASKS")], 100.0), [0.0, ROW]);
}

/// A line the filter hides leaves, a section without a line kept with it;
/// the CPU's and idle's stay, whatever the filter says.
#[test]
fn the_filter_keeps_the_cpu_and_idle() {
    let lines = |shown: &dyn Fn(Line) -> bool| kernel_lines(vec![1, 0], vec![0, 1], vec![0], shown);
    assert_eq!(
        lines(&|_| true),
        [
            Line::Cpu,
            Line::Section("TASKS"),
            Line::Task(Some(1)),
            Line::Task(Some(0)),
            Line::Task(None),
            Line::Section("INTERRUPTS"),
            Line::Handler(0),
            Line::Handler(1),
            Line::Section("MUTEXES"),
            Line::Mutex(0),
        ]
    );
    assert_eq!(
        lines(&|line| matches!(line, Line::Handler(1))),
        [
            Line::Cpu,
            Line::Section("TASKS"),
            Line::Task(None),
            Line::Section("INTERRUPTS"),
            Line::Handler(1),
        ]
    );
    assert_eq!(lines(&|_| false), [Line::Cpu, Line::Section("TASKS"), Line::Task(None)]);
}

/// A wake's arrow joins two lines shown, never one hidden.
#[test]
fn no_arrow_to_a_line_hidden() {
    let lines = [Line::Cpu, Line::Task(Some(0)), Line::Handler(2), Line::Task(None)];
    assert_eq!(wake_rows(&lines, Line::Handler(2), Line::Task(Some(0))), Some((2, 1)));
    assert_eq!(wake_rows(&lines, Line::Handler(1), Line::Task(Some(0))), None);
    assert_eq!(wake_rows(&lines, Line::Handler(2), Line::Task(Some(1))), None);
}
