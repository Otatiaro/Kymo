use super::*;

#[test]
fn a_line_is_found_from_its_top_to_before_the_next() {
    let lines = [
        Line::Cpu,
        Line::Section("TASKS"),
        Line::Task(Some(0)),
        Line::Task(None),
        Line::Event(0),
    ];
    let tops = tops(&lines);
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
    for (n, line) in lines.iter().enumerate() {
        let (top, bottom) = (tops[n], tops[n] + line.height());
        assert_eq!(tops[n + 1], bottom);
        assert_eq!(at(&tops, top), Some(n));
        assert_eq!(at(&tops, bottom - 0.5), Some(n));
    }
    assert_eq!(at(&tops, -0.5), None);
    assert_eq!(at(&tops, tops[lines.len()]), None);
    assert_eq!(at(&super::tops(&[]), 0.0), None);
}
