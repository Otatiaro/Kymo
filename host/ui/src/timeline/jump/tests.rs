use super::*;
use kymo::decode::Decoded;
use kymo::events::{Catalogue, Event};
use kymo::meaning::{Edge, Span};
use kymo::model::Trace;
use std::collections::BTreeMap;

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Each target from `from`, jump after jump, until none is left.
fn walk(starts: &[i64], from: f64, forward: bool) -> Vec<i64> {
    let mut targets = Vec::new();
    let mut at = from;
    while let Some(next) = target(starts, at, forward) {
        targets.push(next);
        at = next as f64;
    }
    targets
}

/// From before the first start, the jumps go through each start once, in
/// order, starts shared by two activities once; from after the last, back.
#[test]
fn jumps_go_through_every_start_once_each_way() {
    let mut state = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..500 {
        let count = random(&mut state) % 12;
        let mut starts: Vec<i64> = (0..count).map(|_| (random(&mut state) % 50) as i64).collect();
        starts.sort_unstable();
        let mut distinct = starts.clone();
        distinct.dedup();
        assert_eq!(walk(&starts, -1.0, true), distinct, "{starts:?}");
        let mut back = distinct.clone();
        back.reverse();
        assert_eq!(walk(&starts, 50.0, false), back, "{starts:?}");
    }
}

/// A start at the reference itself is not a target: a jump always moves.
#[test]
fn a_jump_from_a_start_moves_past_it() {
    let starts = [10, 20, 30];
    assert_eq!(target(&starts, 20.0, true), Some(30));
    assert_eq!(target(&starts, 20.0, false), Some(10));
    assert_eq!(target(&starts, 20.5, true), Some(30));
    assert_eq!(target(&starts, 19.5, false), Some(10));
    assert_eq!(target(&starts, 30.0, true), None);
    assert_eq!(target(&starts, 10.0, false), None);
    assert_eq!(target(&[], 0.0, true), None);
}

fn spans(starts: &[i64]) -> Spans {
    let mut spans = Spans::default();
    for (n, &from) in starts.iter().enumerate() {
        let edge = |cycles| Edge {
            element: 0,
            cycles,
            lost: false,
        };
        spans.push(
            Span {
                from: edge(from),
                to: edge(from + 3),
            },
            n,
        );
    }
    spans
}

/// A handler's interrupts at each depth, and the one still open, are one
/// line of starts.
#[test]
fn the_starts_of_several_rows_and_those_open_merge_in_order() {
    let depths = [spans(&[10, 40, 70]), spans(&[12, 45])];
    assert_eq!(sorted(&depths, [90].into_iter()), [10, 12, 40, 45, 70, 90]);
    assert_eq!(sorted(None, [5, 1].into_iter()), [1, 5]);
}

/// `app::tick` and `app::other`, without fields.
fn catalogue() -> &'static Catalogue {
    let events: BTreeMap<_, _> = ["app::tick", "app::other"]
        .into_iter()
        .zip(0..)
        .map(|(name, id)| {
            let event = Event {
                id,
                name: name.to_owned(),
                size: 0,
                fields: Vec::new(),
                text: None,
            };
            (id, event)
        })
        .collect();
    Box::leak(Box::new(Catalogue {
        events,
        rings: Vec::new(),
        symbols: Default::default(),
        interrupts: Default::default(),
        lines: Default::default(),
        code: Default::default(),
        warnings: Vec::new(),
    }))
}

/// A tick at each of `cycles`, `None` for a loss, `Some(SUSPECT)` for a
/// suspect line.
pub(in crate::timeline) fn timeline(cycles: &[Option<i64>]) -> Timeline {
    let catalogue = catalogue();
    let tick = &catalogue.events[&0];
    let elements = cycles
        .iter()
        .enumerate()
        .map(|(n, cycles)| match *cycles {
            Some(SUSPECT) => Element::Suspect {
                from: 0,
                to: 3,
                writers: 1,
            },
            Some(cycles) => Element::Event {
                event: Decoded {
                    index: n as u32,
                    timestamp: 0,
                    event: tick,
                    payload: Vec::new(),
                },
                cycles,
                seconds: None,
            },
            None => Element::Lost { from: 0, to: 3 },
        })
        .collect();
    let trace = Trace {
        first: 0,
        elements,
        end: None,
        cut: None,
        frequency: None,
        measured: None,
    };
    Timeline::new("test.kcap".to_owned(), catalogue, trace, false).unwrap()
}

const SUSPECT: i64 = -1;

/// The ticks every 10 000 cycles from 1 000, a loss before the first, after
/// the third and after the last; a suspect line after the fifth.
fn ticks() -> Timeline {
    let mut cycles = vec![None];
    for n in 0..6 {
        cycles.push(Some(1000 + 10_000 * n));
        match n {
            2 => cycles.push(None),
            4 => cycles.push(Some(SUSPECT)),
            _ => {}
        }
    }
    cycles.push(None);
    timeline(&cycles)
}

/// The targets of jumps over `starts` from the view `timeline` shows, until
/// the view stops moving.
fn jumps(timeline: &mut Timeline, starts: &[i64], forward: bool) -> Vec<i64> {
    let mut targets = Vec::new();
    loop {
        let before = (timeline.start, timeline.jumped);
        timeline.jump(starts, forward);
        if (timeline.start, timeline.jumped) == before {
            return targets;
        }
        let (target, _, _) = timeline.jumped.expect("a jump");
        targets.push(target);
        let centre = timeline.origin() as f64 + timeline.start + timeline.span / 2.0;
        // Centred, unless an end of the capture holds the view.
        let held = timeline.start == timeline.first() || target as f64 > centre;
        assert!(held || centre == target as f64, "{target} at {centre}");
    }
}

/// Over a narrow view, the jumps go through every event of the line, back
/// from the end then forward from the start, those an end of the capture
/// keeps off centre included; at the same zoom.
#[test]
fn the_jumps_reach_every_event_of_the_line_even_off_centre() {
    let mut timeline = ticks();
    let starts = timeline.starts(Line::Event(0), &Open::default());
    assert_eq!(starts, [1000, 11_000, 21_000, 31_000, 41_000, 51_000]);
    // Wider than the gap between the first event and its neighbour's half.
    timeline.span = 4000.0;
    timeline.start = timeline.last();
    timeline.clamp();
    // The view at the end is centred on the last event already.
    let mut back = starts[..starts.len() - 1].to_vec();
    back.reverse();
    assert_eq!(jumps(&mut timeline, &starts, false), back);
    assert_eq!(timeline.start, timeline.first(), "held at the start");
    assert_eq!(jumps(&mut timeline, &starts, true), starts[1..]);
    assert_eq!(timeline.span, 4000.0);
    assert!(timeline.starts(Line::Event(1), &Open::default()).is_empty());
    assert!(timeline.starts(Line::Cpu, &Open::default()).is_empty());
}

/// Near the start of the capture, where the view cannot centre them, the
/// jumps still go one event at a time, both ways.
#[test]
fn the_jumps_go_one_at_a_time_where_the_view_cannot_centre() {
    let mut timeline = timeline(&[Some(1000), Some(1500), Some(2500), Some(30_000)]);
    let starts = timeline.starts(Line::Event(0), &Open::default());
    timeline.span = 4000.0;
    timeline.start = timeline.first();
    assert_eq!(jumps(&mut timeline, &starts, false), [2500, 1500, 1000]);
    assert_eq!(jumps(&mut timeline, &starts, true), [1500, 2500, 30_000]);
}

/// The losses are those before the first event, between two and after the
/// last, each at the event before it; a suspect line is not one.
#[test]
fn the_losses_start_at_the_event_before_them() {
    let timeline = ticks();
    assert_eq!(timeline.losses(), [1000, 21_000, 51_000]);
}

/// A view moved since the last jump counts from its centre again.
#[test]
fn a_view_moved_counts_from_its_centre() {
    let mut timeline = ticks();
    let starts = timeline.starts(Line::Event(0), &Open::default());
    timeline.span = 4000.0;
    timeline.start = 0.0;
    timeline.jump(&starts, true);
    assert_eq!(timeline.jumped.map(|(target, _, _)| target), Some(11_000));
    timeline.start = 35_000.0;
    timeline.jump(&starts, true);
    assert_eq!(timeline.jumped.map(|(target, _, _)| target), Some(41_000));
}

/// A frame of `timeline` reading `events`, with `modifiers` held, the pointer
/// over `line`.
fn press(timeline: &mut Timeline, events: Vec<egui::Event>, modifiers: egui::Modifiers, line: Option<Line>) {
    let input = egui::RawInput {
        events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
            .chain(events)
            .collect(),
        ..Default::default()
    };
    let _ = egui::Context::default().run_ui(input, |ui| timeline.keys(ui, line, &Open::default()));
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }]
}

/// The arrows go to the next event of the line under the pointer, with
/// shift to the next loss; with control, or off any line, nothing.
#[test]
fn the_arrows_jump_on_the_line_and_shift_on_the_losses() {
    let none = egui::Modifiers::NONE;
    let shift = egui::Modifiers::SHIFT;
    let target = |timeline: &Timeline| timeline.jumped.map(|(target, _, _)| target);
    let narrow = || {
        let mut timeline = ticks();
        timeline.span = 4000.0;
        timeline.start = 5000.0;
        timeline
    };
    // The centre at 7 000 cycles from the origin, 8 000 absolute.
    let mut timeline = narrow();
    press(
        &mut timeline,
        key(egui::Key::ArrowRight, none),
        none,
        Some(Line::Event(0)),
    );
    assert_eq!(target(&timeline), Some(11_000));
    let mut timeline = narrow();
    press(
        &mut timeline,
        key(egui::Key::ArrowLeft, none),
        none,
        Some(Line::Event(0)),
    );
    assert_eq!(target(&timeline), Some(1000));
    let mut timeline = narrow();
    press(
        &mut timeline,
        key(egui::Key::ArrowRight, shift),
        shift,
        Some(Line::Event(0)),
    );
    assert_eq!(target(&timeline), Some(21_000));
    press(&mut timeline, key(egui::Key::ArrowRight, shift), shift, None);
    assert_eq!(target(&timeline), Some(51_000));
    let ctrl = egui::Modifiers::CTRL;
    for (events, modifiers, line) in [
        (key(egui::Key::ArrowRight, none), none, None),
        (key(egui::Key::ArrowRight, ctrl), ctrl, Some(Line::Event(0))),
        (key(egui::Key::ArrowRight, none), none, Some(Line::Event(1))),
    ] {
        let mut timeline = narrow();
        press(&mut timeline, events, modifiers, line);
        assert_eq!(
            (timeline.start, target(&timeline)),
            (5000.0, None),
            "{line:?} {modifiers:?}"
        );
    }
}

/// `F` frames the selection, and nothing without one; `?`, typed, shows the
/// shortcuts and hides them.
#[test]
fn f_frames_and_the_question_mark_lists() {
    let none = egui::Modifiers::NONE;
    let mut timeline = ticks();
    let whole = (timeline.start, timeline.span);
    press(&mut timeline, key(egui::Key::F, none), none, None);
    assert_eq!((timeline.start, timeline.span), whole);
    timeline.selection = Some((11_000, 31_000));
    press(&mut timeline, key(egui::Key::F, none), none, None);
    assert_eq!((timeline.start, timeline.span), (10_000.0, 20_000.0));
    let typed = || vec![egui::Event::Text("?".to_owned())];
    press(&mut timeline, typed(), egui::Modifiers::SHIFT, None);
    assert!(timeline.shortcuts);
    press(&mut timeline, typed(), egui::Modifiers::SHIFT, None);
    assert!(!timeline.shortcuts);
}

/// `F` shows the range selected across the view.
#[test]
fn framing_shows_the_selection_across_the_view() {
    let mut timeline = ticks();
    timeline.frame(11_000, 31_000);
    let origin = timeline.origin() as f64;
    assert_eq!(
        (origin + timeline.start, origin + timeline.start + timeline.span),
        (11_000.0, 31_000.0)
    );
}
