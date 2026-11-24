use super::*;

fn edge(element: usize, cycles: i64) -> Edge {
    Edge {
        element,
        cycles,
        lost: false,
    }
}

fn span(from: (usize, i64), to: (usize, i64)) -> Span {
    Span {
        from: edge(from.0, from.1),
        to: edge(to.0, to.1),
    }
}

/// A task's time ready: a first activation woken at element 0, ready until
/// its run at 20, preempted at 40, back on the core at 60; a second woken at
/// element 12, ready until 150; a third that starts at a loss, at element 30,
/// its time ready known from 310 on.
fn readies() -> (Vec<Ready>, Spans) {
    let first = span((0, 0), (10, 100));
    let second = span((12, 120), (14, 160));
    let third = span((30, 300), (34, 400));
    let readies = vec![
        Ready {
            task: 0,
            span: span((0, 0), (2, 20)),
            open: false,
            activation: first,
            wake: Some(0),
        },
        Ready {
            task: 0,
            span: span((4, 40), (6, 60)),
            open: false,
            activation: first,
            wake: Some(0),
        },
        Ready {
            task: 0,
            span: span((12, 120), (13, 150)),
            open: false,
            activation: second,
            wake: Some(1),
        },
        Ready {
            task: 0,
            span: span((31, 310), (32, 320)),
            open: false,
            activation: third,
            wake: None,
        },
    ];
    let mut spans = Spans::default();
    for (n, ready) in readies.iter().enumerate() {
        spans.push(ready.span, n);
    }
    (readies, spans)
}

/// A run reads the time ready it ends, in its own activation: after a
/// preemption, the activation's; never a neighbouring activation's time ready
/// that reaches the same cycles.
#[test]
fn a_run_reads_the_time_ready_of_its_own_activation() {
    let (readies, spans) = readies();
    assert_eq!(before_run(&readies, Some(&spans), edge(2, 20)), Some(0));
    assert_eq!(before_run(&readies, Some(&spans), edge(6, 60)), Some(1));
    assert_eq!(before_run(&readies, Some(&spans), edge(13, 150)), Some(2));
    // At cycles the second activation's time ready reaches, a run of another.
    assert_eq!(before_run(&readies, Some(&spans), edge(16, 150)), None);
    // No time ready before it.
    assert_eq!(before_run(&readies, Some(&spans), edge(8, 80)), None);
    assert_eq!(before_run(&readies, None, edge(2, 20)), None);
}

/// The wait is the activation's first time ready, from any piece of it; none
/// when the activation starts at a loss before its time ready.
#[test]
fn the_wait_is_the_first_time_ready_of_the_activation() {
    let (readies, _) = readies();
    let waited = |which| waited(&readies, which).map(|ready| ready.span);
    assert_eq!(waited(0), Some(readies[0].span));
    assert_eq!(waited(1), Some(readies[0].span));
    assert_eq!(waited(2), Some(readies[2].span));
    assert_eq!(waited(3), None);
}
