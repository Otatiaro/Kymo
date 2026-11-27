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

#[test]
fn what_happened_in_a_context_is_marked_on_its_task_or_handler_else_on_the_cpu() {
    let lines = [Line::Cpu, Line::Task(Some(0)), Line::Task(None), Line::Handler(1)];
    assert_eq!(context_row(&lines, Context::Task(Some(0))), Some(1));
    assert_eq!(context_row(&lines, Context::Task(None)), Some(2));
    assert_eq!(context_row(&lines, Context::Handler(1)), Some(3));
    // Its row not shown, or none to show.
    assert_eq!(context_row(&lines, Context::Task(Some(4))), Some(0));
    assert_eq!(context_row(&lines, Context::Exception(11)), Some(0));
    assert_eq!(context_row(&lines, Context::Unknown), Some(0));
    assert_eq!(context_row(&lines[1..], Context::Unknown), None);
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

/// Tasks group by namespace, a namespace's own before its nested ones', and
/// keep the order of their priority within each.
#[test]
fn tasks_are_by_namespace_then_by_priority() {
    let names = ["app::g_b", "drv::g_c", "app::imu::g_d", "app::g_a", "g_e"];
    let priority = [1, 0, 0, 2, 5];
    let order = by_namespace_then(&names, |n| priority[n]);
    let order: Vec<&str> = order.iter().map(|&n| names[n]).collect();
    assert_eq!(order, ["g_e", "app::g_b", "app::g_a", "app::imu::g_d", "drv::g_c"]);
}

/// An image whose kernel adapter is `isr_enter` and `isr_exit` alone, their
/// ids 1 and 2.
fn adapted() -> &'static kymo::events::Catalogue {
    use kymo::events::{Catalogue, Event, Field, Kind, Pointee, Type};
    let field = |name: &str, offset, kind| Field {
        name: name.to_owned(),
        offset,
        ty: Type {
            name: name.to_owned(),
            size: Some(4),
            kind,
        },
    };
    let handler = || field("handler", 0, Kind::Pointer(Pointee::Function));
    let exception = field("exception", 4, Kind::Base(gimli::DwAte(gimli::DW_ATE_unsigned.0)));
    let event = |id, hook: &str, fields: Vec<Field>| Event {
        id,
        name: format!("kymo::opsy::{hook}"),
        size: 4 * fields.len() as u64,
        fields,
        text: None,
    };
    let enter = event(1, "isr_enter", vec![handler(), exception]);
    let exit = event(2, "isr_exit", vec![handler()]);
    Box::leak(Box::new(Catalogue {
        events: [(1, enter), (2, exit)].into_iter().collect(),
        rings: Vec::new(),
        symbols: Default::default(),
        interrupts: Default::default(),
        lines: Default::default(),
        code: Default::default(),
        warnings: Vec::new(),
    }))
}

/// A trace of SWO alone spans its ticks, its exceptions on their handler's
/// line, the rest of the time the task unknown.
#[test]
fn a_trace_of_swo_alone_shows_its_exceptions_within_its_ticks() {
    use kymo::itm::Function;
    use kymo::swo::{Exception, Watched};
    let exception = |function, cycles| Exception {
        number: 15,
        function,
        cycles,
    };
    let watched = Watched {
        span: Some((1_000, 5_000)),
        exceptions: vec![
            exception(Function::Entered, 2_000),
            exception(Function::Exited, 2_100),
            exception(Function::Entered, 3_000),
            exception(Function::Exited, 3_300),
        ],
        ..Watched::default()
    };
    let trace = kymo::model::Trace {
        first: 0,
        elements: Vec::new(),
        end: None,
        cut: None,
        watched: Some(watched),
        frequency: None,
        measured: None,
    };
    let timeline = Timeline::new("swo.kcap".to_owned(), adapted(), trace, false).unwrap();
    assert_eq!((timeline.origin, timeline.bounds), (Some(1_000), Some((1_000, 5_000))));
    let meaning = timeline.meaning.as_ref().expect("a kernel adapter");
    let handler = meaning
        .interrupts
        .iter()
        .position(|depths| !depths.is_empty())
        .expect("a handler");
    assert!(timeline.lines().contains(&Line::Handler(handler)));
    let total = |covered: &kymo::load::Covered| covered.until(i64::MAX);
    assert_eq!(total(&meaning.load.handlers[handler]), 400);
    assert_eq!(total(&meaning.load.unknown), 3_600);
}

/// The exceptions replace the handlers' spans the events gave: each span of
/// the kernel on its row once.
#[test]
fn the_exceptions_replace_the_spans_of_the_events() {
    use kymo::decode::Decoded;
    use kymo::itm::Function;
    use kymo::model::Element;
    use kymo::swo::{Exception, Watched};
    let catalogue = adapted();
    let element = |index: u32, id: u16, payload: &[u32], cycles| Element::Event {
        event: Decoded {
            index,
            timestamp: 0,
            event: &catalogue.events[&id],
            payload: payload.iter().flat_map(|word| word.to_le_bytes()).collect(),
        },
        cycles,
        seconds: None,
    };
    const HANDLER: u32 = 0x0800_0101;
    let elements = vec![
        element(0, 1, &[HANDLER, 15], 100),
        element(1, 2, &[HANDLER], 300),
        element(2, 1, &[HANDLER, 15], 2_000),
        element(3, 2, &[HANDLER], 2_200),
    ];
    let exception = |function, cycles| Exception {
        number: 15,
        function,
        cycles,
    };
    let watched = Watched {
        span: Some((1_000, 5_000)),
        exceptions: vec![exception(Function::Entered, 2_000), exception(Function::Exited, 2_100)],
        ..Watched::default()
    };
    let trace = kymo::model::Trace {
        first: 0,
        elements,
        end: None,
        cut: None,
        watched: Some(watched),
        frequency: None,
        measured: None,
    };
    let timeline = Timeline::new("both.kcap".to_owned(), catalogue, trace, false).unwrap();
    let meaning = timeline.meaning.as_ref().expect("a kernel adapter");
    let placed: usize = meaning
        .interrupts
        .iter()
        .flatten()
        .map(|spans| spans.starts().len())
        .sum();
    assert_eq!(placed, meaning.kernel.closed.interrupts.len());
    assert!(placed > 0);
}

/// A wake's arrow joins two lines shown, never one hidden.
#[test]
fn no_arrow_to_a_line_hidden() {
    let lines = [Line::Cpu, Line::Task(Some(0)), Line::Handler(2), Line::Task(None)];
    assert_eq!(wake_rows(&lines, Line::Handler(2), Line::Task(Some(0))), Some((2, 1)));
    assert_eq!(wake_rows(&lines, Line::Handler(1), Line::Task(Some(0))), None);
    assert_eq!(wake_rows(&lines, Line::Handler(2), Line::Task(Some(1))), None);
}
