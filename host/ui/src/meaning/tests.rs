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

fn subject(address: u64, name: &str) -> Subject {
    Subject {
        address,
        name: name.to_owned(),
        priority: None,
        priorities: Vec::new(),
    }
}

fn instance(kind: &'static str, subject: &str, detail: Option<&str>) -> Instance {
    Instance {
        kind,
        subject: subject.to_owned(),
        metric: kymo::stats::Metric {
            name: "net",
            share: true,
            jitter: false,
        },
        detail: detail.map(str::to_owned),
        value: 0,
        from: edge(0, 0),
        to: edge(1, 1),
        open: false,
        swo: false,
    }
}

/// A task and a handler at their addresses, and a mutex no event field
/// gives, at address 0.
fn subjects() -> [Vec<Subject>; 3] {
    [
        vec![subject(0x2000_0010, "app::(anonymous namespace)::g_sampler")],
        vec![subject(0x0800_0101, "opsy::notify_one+0x88")],
        vec![subject(0, "app::g_mutex")],
    ]
}

fn names() -> [Vec<String>; 3] {
    [
        vec!["g_sampler".to_owned()],
        vec!["notify_one+0x88".to_owned()],
        vec!["g_mutex".to_owned()],
    ]
}

/// A field pointing at a subject's address reads its name shown; at any
/// other address, or at 0, none.
#[test]
fn a_field_names_a_subject_by_its_address() {
    let (subjects, names) = (subjects(), names());
    let named: Named = [
        ("task", &subjects[0], &names[0]),
        ("handler", &subjects[1], &names[1]),
        ("mutex", &subjects[2], &names[2]),
    ];
    assert_eq!(shown_at(&named, 0x2000_0010), Some("g_sampler"));
    assert_eq!(shown_at(&named, 0x0800_0101), Some("notify_one+0x88"));
    assert_eq!(shown_at(&named, 0x2000_0020), None);
    assert_eq!(shown_at(&named, 0), None);
}

/// The statistics show a subject, and the subject a cause names, by the
/// name its row shows, as does a mutex the task holding or waiting, the name
/// in full kept for the hover; another kind's
/// subject of the same name, a reason and idle stay as they are.
#[test]
fn the_statistics_name_subjects_as_their_rows() {
    let (subjects, names) = (subjects(), names());
    let named: Named = [
        ("task", &subjects[0], &names[0]),
        ("handler", &subjects[1], &names[1]),
        ("mutex", &subjects[2], &names[2]),
    ];
    let full_task = "app::(anonymous namespace)::g_sampler";
    let full_handler = "opsy::notify_one+0x88";
    let (instances, full) = shorten(
        &named,
        vec![
            instance("task", full_task, Some(&format!("handler:{full_handler}"))),
            instance("task", full_task, Some("notified")),
            instance("task", full_task, Some("task:idle")),
            instance("mutex", full_task, None),
            instance("mutex", "app::g_mutex", Some(full_task)),
        ],
    );
    let shown: Vec<(&str, Option<&str>)> = instances
        .iter()
        .map(|instance| (instance.subject.as_str(), instance.detail.as_deref()))
        .collect();
    assert_eq!(
        shown,
        [
            ("g_sampler", Some("handler:notify_one+0x88")),
            ("g_sampler", Some("notified")),
            ("g_sampler", Some("task:idle")),
            (full_task, None),
            ("g_mutex", Some("g_sampler")),
        ]
    );
    assert_eq!(full[&("task", "g_sampler".to_owned())], full_task);
    assert_eq!(
        full[&("detail", "handler:notify_one+0x88".to_owned())],
        format!("handler:{full_handler}")
    );
    assert_eq!(full[&("detail", "g_sampler".to_owned())], full_task);
    assert_eq!(full[&("mutex", "g_mutex".to_owned())], "app::g_mutex");
    assert_eq!(full.len(), 4);
}

/// Application events go each to its context's cycles and elements, taken
/// in in pieces as with a capture that grows.
#[test]
fn application_events_are_kept_by_the_context_that_emitted_them() {
    let instant = |element: usize, context: Context| Instant {
        edge: edge(element, 10 * element as i64),
        context,
    };
    let a = Context::Task(Some(0));
    let h = Context::Handler(0);
    let mut contexts = Vec::new();
    by_context(&mut contexts, &[instant(1, a), instant(2, h), instant(3, a)]);
    by_context(
        &mut contexts,
        &[instant(5, Context::Unknown), instant(6, h), instant(7, a)],
    );
    let kept: Vec<_> = contexts
        .iter()
        .map(|(context, levels, elements)| (*context, levels.cycles().to_vec(), elements.clone()))
        .collect();
    assert_eq!(
        kept,
        [
            (a, vec![10, 30, 70], vec![1, 3, 7]),
            (h, vec![20, 60], vec![2, 6]),
            (Context::Unknown, vec![50], vec![5]),
        ]
    );
}
