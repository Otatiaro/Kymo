use super::*;

const ENTERED: Function = Function::Entered;
const EXITED: Function = Function::Exited;
const RETURNED: Function = Function::Returned;

fn packets(list: &[(u16, Function, i64)]) -> Vec<Exception> {
    list.iter()
        .map(|&(number, function, cycles)| Exception {
            number,
            function,
            cycles,
        })
        .collect()
}

fn at(cycles: i64) -> At {
    At { cycles, lost: false }
}

fn lost(cycles: i64) -> At {
    At { cycles, lost: true }
}

fn line(exception: u16, depth: Option<usize>, from: At, to: At) -> Line {
    Line {
        exception,
        depth,
        from,
        to,
    }
}

#[test]
fn entries_exits_and_returns_nest() {
    // TIM2 (61) from thread mode, interrupted by 62, then a tail chain from
    // TIM2 to PendSV (14), and back to thread mode.
    let exceptions = packets(&[
        (0, RETURNED, 10),
        (61, ENTERED, 100),
        (62, ENTERED, 150),
        (62, EXITED, 200),
        (61, RETURNED, 200),
        (61, EXITED, 300),
        (14, ENTERED, 310),
        (14, EXITED, 400),
        (0, RETURNED, 400),
    ]);
    let nested = nest(&exceptions, &[(0, 1000)]);
    assert_eq!(
        nested.lines,
        [
            line(62, Some(1), at(150), at(200)),
            line(61, Some(0), at(100), at(300)),
            line(14, Some(0), at(310), at(400)),
        ]
    );
    assert!(nested.contradictions.is_empty());
}

#[test]
fn the_nesting_is_unknown_until_a_return_to_thread_mode() {
    // The stretch starts inside 61: its exit has no entry, and 62's depth is
    // unknown until the return to thread mode.
    let exceptions = packets(&[
        (62, ENTERED, 120),
        (62, EXITED, 130),
        (61, RETURNED, 130),
        (61, EXITED, 200),
        (0, RETURNED, 200),
        (62, ENTERED, 300),
        (62, EXITED, 310),
        (0, RETURNED, 310),
    ]);
    let nested = nest(&exceptions, &[(100, 1000)]);
    assert_eq!(
        nested.lines,
        [
            line(62, None, at(120), at(130)),
            line(61, None, lost(100), at(200)),
            line(62, Some(0), at(300), at(310)),
        ]
    );
    assert!(nested.contradictions.is_empty());
}

#[test]
fn a_gap_ends_the_handlers_open_and_hides_the_nesting() {
    // 61 is open at the gap from 500 to 600; after it, 61's exit has no entry.
    // The entry of 62 in the gap is dated by no anchor.
    let exceptions = packets(&[
        (0, RETURNED, 10),
        (61, ENTERED, 400),
        (62, ENTERED, 550),
        (61, EXITED, 650),
        (0, RETURNED, 650),
        (61, ENTERED, 700),
        (61, EXITED, 720),
    ]);
    let nested = nest(&exceptions, &[(0, 500), (600, 1000)]);
    assert_eq!(
        nested.lines,
        [
            line(61, Some(0), at(400), lost(500)),
            line(61, None, lost(600), at(650)),
            line(61, Some(0), at(700), at(720)),
        ]
    );
}

#[test]
fn a_handler_open_at_the_last_anchor_ends_in_what_follows() {
    let exceptions = packets(&[(0, RETURNED, 10), (61, ENTERED, 900)]);
    assert_eq!(
        nest(&exceptions, &[(0, 1000)]).lines,
        [line(61, Some(0), at(900), lost(1000))]
    );
}

#[test]
fn packets_against_the_nesting_are_contradictions() {
    // An exit of an exception not open, a return to one not open, a return to
    // thread mode with one open.
    let exceptions = packets(&[
        (0, RETURNED, 10),
        (61, EXITED, 100),
        (61, ENTERED, 200),
        (62, RETURNED, 210),
        (0, RETURNED, 300),
    ]);
    let nested = nest(&exceptions, &[(0, 1000)]);
    assert!(nested.lines.is_empty());
    assert_eq!(nested.contradictions, [100, 210, 300]);
}

#[test]
fn the_coverage_is_the_span_less_its_gaps() {
    let watched = Watched {
        span: Some((100, 1000)),
        gaps: vec![(100, 200), (500, 600)],
        ..Watched::default()
    };
    assert_eq!(coverage(&watched), [(200, 500), (600, 1000)]);
    assert!(coverage(&Watched::default()).is_empty());
}
