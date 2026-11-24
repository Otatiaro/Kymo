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
