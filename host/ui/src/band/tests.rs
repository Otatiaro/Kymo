use super::*;

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// The runs and interrupts, each with whether it is still open, and the
/// capture's first and last events.
type Spanned = (Vec<(Run, bool)>, Vec<(Interrupt, bool)>, Option<(Edge, Edge)>);

/// What the core does from an event's cycle on.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// To a task, the idle task for none.
    Switch(Option<usize>),
    Enter(usize),
    Exit,
    Loss,
}

/// The runs and interrupts a kernel's meaning makes of `steps`, each at its
/// cycles, the element its place: a loss ends the run and the handlers open;
/// those open at the last step end there, still open. And the first step's
/// edge and the last's.
fn spans(steps: &[(i64, Step)]) -> Spanned {
    let edge = |element: usize, lost: bool| Edge {
        element,
        cycles: steps[element].0,
        lost,
    };
    let mut runs = Vec::new();
    let mut interrupts = Vec::new();
    let mut running: Option<(Option<usize>, Edge)> = None;
    let mut entered: Vec<(usize, Edge, Option<usize>)> = Vec::new();
    // Whether the handlers open are known: not since a loss.
    let mut depths = true;
    for (n, &(_, step)) in steps.iter().enumerate() {
        match step {
            Step::Switch(task) => {
                if let Some((was, from)) = running.take() {
                    runs.push((
                        Run {
                            task: was,
                            span: Span {
                                from,
                                to: edge(n, false),
                            },
                        },
                        false,
                    ));
                }
                running = Some((task, edge(n, false)));
            }
            Step::Enter(handler) => {
                let depth = depths.then_some(entered.len());
                entered.push((handler, edge(n, false), depth));
            }
            Step::Exit => {
                if let Some((handler, from, depth)) = entered.pop() {
                    let span = Span {
                        from,
                        to: edge(n, false),
                    };
                    interrupts.push((Interrupt { handler, depth, span }, false));
                }
            }
            Step::Loss => {
                if let Some((task, from)) = running.take() {
                    let span = Span {
                        from,
                        to: edge(n, true),
                    };
                    runs.push((Run { task, span }, false));
                }
                for (handler, from, depth) in entered.drain(..).rev() {
                    let span = Span {
                        from,
                        to: edge(n, true),
                    };
                    interrupts.push((Interrupt { handler, depth, span }, false));
                }
                depths = false;
            }
        }
    }
    let last = steps.len() - 1;
    if let Some((task, from)) = running {
        let span = Span {
            from,
            to: edge(last, false),
        };
        runs.push((Run { task, span }, true));
    }
    for (handler, from, depth) in entered.into_iter().rev() {
        let span = Span {
            from,
            to: edge(last, false),
        };
        interrupts.push((Interrupt { handler, depth, span }, true));
    }
    (runs, interrupts, Some((edge(0, false), edge(last, false))))
}

/// What holds the core at each cycle from the first step to the last, the
/// steps walked one cycle at a time with a stack of the handlers entered,
/// none for idle.
fn oracle(steps: &[(i64, Step)]) -> Vec<Option<Held>> {
    let (first, last) = (steps[0].0, steps[steps.len() - 1].0);
    let mut held = Vec::new();
    // The task on the core: None unknown, Some(None) idle.
    let mut task: Option<Option<usize>> = None;
    // The handlers entered, with the task on the core when each was.
    let mut stack: Vec<(usize, Option<Option<usize>>)> = Vec::new();
    let mut next = 0;
    for cycle in first..last {
        while next < steps.len() && steps[next].0 == cycle {
            match steps[next].1 {
                Step::Switch(to) => task = Some(to),
                Step::Enter(handler) => stack.push((handler, task)),
                Step::Exit => {
                    stack.pop();
                }
                Step::Loss => {
                    task = None;
                    stack.clear();
                }
            }
            next += 1;
        }
        held.push(match (stack.last(), task) {
            (Some(&(handler, entered)), _) => {
                // A handler interrupts the one it nests in, else the task on
                // the core when it was entered.
                let under = match stack.len().checked_sub(2) {
                    Some(outer) => Under::Handler(stack[outer].0),
                    None => entered.map_or(Under::Unknown, Under::Task),
                };
                Some(Held::Handler { handler, under })
            }
            (None, Some(Some(task))) => Some(Held::Task(task)),
            (None, Some(None)) => None,
            (None, None) => Some(Held::Unknown),
        });
    }
    held
}

/// Steps at increasing cycles: switches, handlers nested up to three deep,
/// losses; some steps a cycle apart, some far.
fn steps(state: &mut u64) -> Vec<(i64, Step)> {
    let mut at = (random(state) % 100) as i64;
    let mut depth = 0;
    let mut steps = Vec::new();
    for _ in 0..2 + random(state) % 60 {
        let step = match random(state) % 10 {
            0..=2 => Step::Switch((!random(state).is_multiple_of(3)).then(|| (random(state) % 4) as usize)),
            3..=5 if depth < 3 => Step::Enter((random(state) % 3) as usize),
            6..=8 if depth > 0 => Step::Exit,
            9 if random(state).is_multiple_of(3) => Step::Loss,
            _ => Step::Switch(Some((random(state) % 4) as usize)),
        };
        depth = match step {
            Step::Enter(_) => depth + 1,
            Step::Exit => depth - 1,
            Step::Loss => 0,
            Step::Switch(_) => depth,
        };
        steps.push((at, step));
        at += 1 + (random(state) % if random(state).is_multiple_of(4) { 200 } else { 5 }) as i64;
    }
    steps
}

/// What the band holds at each cycle from the first step to the last; no
/// two pieces hold the same one.
fn per_cycle(band: &Band, steps: &[(i64, Step)]) -> Vec<Option<Held>> {
    let (first, last) = (steps[0].0, steps[steps.len() - 1].0);
    let mut held = vec![None; (last - first) as usize];
    for piece in &band.pieces {
        assert!(piece.from < piece.to, "{piece:?}");
        assert!(first <= piece.from && piece.to <= last, "{piece:?}");
        for cycle in piece.from..piece.to {
            let at = &mut held[(cycle - first) as usize];
            assert!(at.is_none(), "two pieces at {cycle}");
            *at = Some(piece.held);
        }
    }
    held
}

/// At every cycle the band shows what the core holds as a walk of the steps
/// gives it: the innermost handler and what it interrupts, else the task,
/// nothing for idle, the time unknown from a loss or the start to a switch.
#[test]
fn the_band_holds_at_each_cycle_what_a_walk_of_the_steps_gives() {
    let mut state = 0x2545_f491_4f6c_dd1d;
    let mut nested = 0;
    let mut unknown = 0;
    for _ in 0..3000 {
        let steps = steps(&mut state);
        let (runs, interrupts, start) = spans(&steps);
        let band = Band::of(&runs, &interrupts, start);
        let expected = oracle(&steps);
        let held = per_cycle(&band, &steps);
        if let Some(n) = (0..expected.len()).find(|&n| held[n] != expected[n]) {
            panic!(
                "at {}: {:?}, not {:?}\n{steps:?}",
                steps[0].0 + n as i64,
                held[n],
                expected[n]
            );
        }
        nested += expected
            .iter()
            .filter(|held| {
                matches!(
                    held,
                    Some(Held::Handler {
                        under: Under::Handler(_),
                        ..
                    })
                )
            })
            .count();
        unknown += expected.iter().filter(|held| **held == Some(Held::Unknown)).count();
        // Each piece's span is that of what it shows, and holds it.
        for piece in &band.pieces {
            assert!(
                piece.span.from.cycles <= piece.from && piece.to <= piece.span.to.cycles,
                "{piece:?}"
            );
        }
    }
    assert!(nested > 1000 && unknown > 1000, "{nested} nested, {unknown} unknown");
}

fn edge(cycles: i64, lost: bool) -> Edge {
    Edge {
        element: 0,
        cycles,
        lost,
    }
}

/// A capture from cycle 0 to `end`.
fn bounds(end: i64) -> Option<(Edge, Edge)> {
    Some((edge(0, false), edge(end, false)))
}

fn run(task: Option<usize>, from: i64, to: i64, lost: bool) -> (Run, bool) {
    let span = Span {
        from: edge(from, false),
        to: edge(to, lost),
    };
    (Run { task, span }, false)
}

fn interrupt(handler: usize, from: i64, to: i64) -> (Interrupt, bool) {
    let span = Span {
        from: edge(from, false),
        to: edge(to, false),
    };
    (
        Interrupt {
            handler,
            depth: None,
            span,
        },
        false,
    )
}

fn shown(band: &Band) -> Vec<(i64, i64, Held)> {
    band.pieces
        .iter()
        .map(|piece| (piece.from, piece.to, piece.held))
        .collect()
}

#[test]
fn a_handler_cuts_the_task_it_interrupts_and_shows_over_idle() {
    let band = Band::of(
        &[run(Some(0), 0, 100, false), run(None, 100, 200, false)],
        &[interrupt(5, 40, 50), interrupt(6, 150, 160)],
        bounds(200),
    );
    assert_eq!(
        shown(&band),
        [
            (0, 40, Held::Task(0)),
            (
                40,
                50,
                Held::Handler {
                    handler: 5,
                    under: Under::Task(Some(0))
                }
            ),
            (50, 100, Held::Task(0)),
            (
                150,
                160,
                Held::Handler {
                    handler: 6,
                    under: Under::Task(None)
                }
            ),
        ]
    );
}

#[test]
fn nested_handlers_show_the_inner_one_over_the_outer() {
    let band = Band::of(
        &[run(Some(1), 0, 100, false)],
        &[interrupt(2, 20, 25), interrupt(1, 10, 40)],
        bounds(100),
    );
    let outer = Held::Handler {
        handler: 1,
        under: Under::Task(Some(1)),
    };
    let inner = Held::Handler {
        handler: 2,
        under: Under::Handler(1),
    };
    assert_eq!(
        shown(&band),
        [
            (0, 10, Held::Task(1)),
            (10, 20, outer),
            (20, 25, inner),
            (25, 40, outer),
            (40, 100, Held::Task(1)),
        ]
    );
}

#[test]
fn the_time_after_a_loss_is_unknown_until_a_switch() {
    let band = Band::of(
        &[run(Some(0), 10, 50, true), run(Some(1), 80, 90, false)],
        &[],
        bounds(90),
    );
    assert_eq!(
        shown(&band),
        [
            (0, 10, Held::Unknown),
            (10, 50, Held::Task(0)),
            (50, 80, Held::Unknown),
            (80, 90, Held::Task(1)),
        ]
    );
    // The time unknown starts at the loss, the first at the capture's start.
    assert!(band.pieces[2].span.from.lost);
    assert!(!band.pieces[0].span.from.lost);
}

#[test]
fn spans_still_open_end_at_the_last_event() {
    let (mut task, mut handler) = (run(Some(3), 0, 100, false), interrupt(0, 60, 100));
    task.1 = true;
    handler.1 = true;
    let band = Band::of(&[task], &[handler], bounds(100));
    assert_eq!(band.pieces.len(), 2);
    assert!(band.pieces.iter().all(|piece| piece.still));
}

fn view(span: f64, width: f64) -> View {
    View {
        origin: 0,
        start: 0.0,
        span,
        left: 100.0,
        width,
    }
}

fn layer(held: Held) -> Layer {
    match held {
        Held::Unknown => Layer::Unknown,
        Held::Task(task) => Layer::Task(task),
        Held::Handler { .. } => Layer::Handlers,
    }
}

#[test]
fn a_piece_shorter_than_a_pixel_keeps_one() {
    // A thousand cycles a pixel: a handler of one cycle still shows.
    let band = Band::of(
        &[run(Some(0), 0, 100_000, false)],
        &[interrupt(0, 50_000, 50_001)],
        bounds(100_000),
    );
    let view = view(100_000.0, 100.0);
    let x = view.column(50_000);
    assert!(matches!(
        band.at(&view, x).map(|piece| piece.held),
        Some(Held::Handler { .. })
    ));
    assert_eq!(band.at(&view, x + 1.0).map(|piece| piece.held), Some(Held::Task(0)));
}

/// The pointer reads, at each pixel column, a piece of the last layer drawn
/// there, at zooms from a cycle to thousands per pixel.
#[test]
fn the_pointer_reads_the_piece_drawn_on_top() {
    let mut state = 0x9e37_79b9_7f4a_7c15;
    let mut covered = 0;
    for _ in 0..500 {
        let steps = steps(&mut state);
        let (runs, interrupts, start) = spans(&steps);
        let band = Band::of(&runs, &interrupts, start);
        let last = steps[steps.len() - 1].0 as f64;
        let view = View {
            origin: 0,
            start: (random(&mut state) % 50) as f64,
            span: 1.0 + last * (random(&mut state) % 100) as f64 / 100.0,
            left: 100.0,
            width: 50.0 + (random(&mut state) % 400) as f64,
        };
        let drawn: Vec<(Layer, Vec<crate::spans::Band>)> = band
            .layers()
            .map(|(layer, spans)| (layer, spans.bands(&view)))
            .collect();
        let right = view.left + view.width as f32;
        let mut x = view.left;
        while x < right {
            let top = drawn
                .iter()
                .rev()
                .find(|(_, bands)| bands.iter().any(|band| band.from <= x && x < band.to))
                .map(|(layer, _)| *layer);
            assert_eq!(band.at(&view, x).map(|piece| layer(piece.held)), top, "{x} {steps:?}");
            covered += usize::from(top.is_some());
            x += 1.0;
        }
    }
    assert!(covered > 10_000, "{covered}");
}
