use super::*;
use kymo::decode::Decoded;
use kymo::events::{Event, Field, Kind, Type};
use kymo::model::Edge;
use kymo::values::Sampled;
use std::collections::BTreeMap;

/// `app::setpoint {int32_t x; float f;}`, `app::tick` with no field, and
/// kymo's own `kymo::begin_of<app::batch> {uint32_t id;}`.
fn catalogue() -> Catalogue {
    let base = |name: &str, size, encoding| Type {
        name: name.to_owned(),
        size: Some(size),
        kind: Kind::Base(encoding),
    };
    let field = |name: &str, offset, ty| Field {
        name: name.to_owned(),
        offset,
        ty,
    };
    let setpoint = vec![
        field("x", 0, base("int32_t", 4, gimli::DW_ATE_signed)),
        field("f", 4, base("float", 4, gimli::DW_ATE_float)),
    ];
    let batch = vec![field("id", 0, base("uint32_t", 4, gimli::DW_ATE_unsigned))];
    let events: BTreeMap<_, _> = [
        ("app::setpoint", 8, setpoint),
        ("app::tick", 0, Vec::new()),
        ("kymo::begin_of<app::batch>", 4, batch),
    ]
    .into_iter()
    .zip(0..)
    .map(|((name, size, fields), id)| {
        let event = Event {
            id,
            name: name.to_owned(),
            size,
            fields,
            text: None,
        };
        (id, event)
    })
    .collect();
    Catalogue {
        events,
        rings: Vec::new(),
        symbols: Default::default(),
        interrupts: Default::default(),
        lines: Default::default(),
        code: Default::default(),
        warnings: Vec::new(),
    }
}

/// What a test's trace holds, in order.
enum Step {
    Setpoint(i64, i32, f32),
    Tick(i64),
    Lost,
}

fn elements<'a>(catalogue: &'a Catalogue, steps: &[Step]) -> Vec<Element<'a>> {
    let event = |name: &str| catalogue.events.values().find(|event| event.name == name).unwrap();
    steps
        .iter()
        .enumerate()
        .map(|(n, step)| {
            let (name, cycles, payload) = match *step {
                Step::Setpoint(cycles, x, f) => {
                    let mut payload = x.to_le_bytes().to_vec();
                    payload.extend(f.to_le_bytes());
                    ("app::setpoint", cycles, payload)
                }
                Step::Tick(cycles) => ("app::tick", cycles, Vec::new()),
                Step::Lost => return Element::Lost { from: 0, to: 3 },
            };
            Element::Event {
                event: Decoded {
                    index: n as u32,
                    timestamp: 0,
                    event: event(name),
                    payload,
                },
                cycles,
                seconds: None,
            }
        })
        .collect()
}

/// A view of `span` cycles from 0 over `width` pixels from 0.
fn view(span: f64, width: f64) -> View {
    View {
        origin: 0,
        start: 0.0,
        span,
        left: 0.0,
        width,
    }
}

fn sample(cycles: i64, value: i128) -> Sample {
    Sample {
        cycles,
        element: 0,
        value: Number::Integer(value),
        cut: None,
    }
}

fn bar(from: f32, to: f32, low: f64, high: f64) -> Bar {
    Bar { from, to, low, high }
}

#[test]
fn the_curves_are_the_numeric_fields_of_application_events() {
    let catalogue = catalogue();
    let values = Values::new(&catalogue, None).unwrap();
    let names: Vec<String> = values.curves.iter().map(Curve::to_string).collect();
    assert_eq!(names, ["app::setpoint.f", "app::setpoint.x"]);
    assert_eq!(values.named("app::setpoint.x"), Some(1));
    assert_eq!(values.named("app::tick.x"), None);
}

/// A loss cuts the step of each curve's last sample at the last event
/// before it, whichever type that is; the next sample is not cut.
#[test]
fn a_loss_cuts_the_last_step_of_every_curve() {
    let catalogue = catalogue();
    let steps = [
        Step::Setpoint(0, 1, 0.5),
        Step::Setpoint(10, 2, 0.5),
        Step::Tick(20),
        Step::Lost,
        Step::Lost,
        Step::Tick(50),
        Step::Setpoint(100, 3, 0.5),
    ];
    let elements = elements(&catalogue, &steps);
    let mut values = Values::new(&catalogue, None).unwrap();
    values.add(&elements[..4], 0);
    values.add(&elements, 4);
    let x = values.named("app::setpoint.x").unwrap();
    let cuts: Vec<(i64, Option<i64>)> = values.samples[x]
        .iter()
        .map(|sample| (sample.cycles, sample.cut))
        .collect();
    assert_eq!(cuts, [(0, None), (10, Some(20)), (100, None)]);
    assert_eq!(values.samples[x][2].element, 6);
    // Held, but unknown past the loss.
    let held = values.held(x, 30).unwrap();
    assert_eq!(
        (held.value, held.holds_at(20), held.holds_at(30)),
        (Number::Integer(2), true, false)
    );
    assert!(values.held(x, -1).is_none());
    // Drawn: no step from the loss to the next sample, nor past the last
    // event.
    assert_eq!(values.end, 100);
    let drawn = drawn(&values.samples[x], values.end, &view(200.0, 200.0));
    assert_eq!(
        drawn.bars,
        [
            bar(0.0, 10.0, 1.0, 1.0),
            bar(10.0, 11.0, 1.0, 2.0),
            bar(11.0, 21.0, 2.0, 2.0),
            bar(100.0, 101.0, 3.0, 3.0)
        ]
    );
}

/// Zoomed out, a sample alone in its column, far from the others' values,
/// still draws its column to it: a peak of one sample is never lost.
#[test]
fn a_peak_of_one_sample_stays_visible_zoomed_out() {
    for peak_at in [0, 1, 4_999, 50_000, 99_999] {
        let samples: Vec<Sample> = (0..100_000)
            .map(|n| sample(n * 7, if n == peak_at { 1000 } else { (n % 3) as i128 }))
            .collect();
        let drawn = drawn(&samples, 700_000, &view(700_000.0, 1000.0));
        assert_eq!((drawn.low, drawn.high), (0.0, 1000.0), "peak at {peak_at}");
        let column = (peak_at * 7 * 1000 / 700_000) as f32;
        let peak = drawn
            .bars
            .iter()
            .find(|bar| bar.from <= column && column < bar.to)
            .unwrap();
        assert_eq!(peak.high, 1000.0, "peak at {peak_at}");
        // Zoomed out, no dot.
        assert!(drawn.dots.is_empty());
    }
}

/// Each column shows the step entering it and the samples in it: the lowest
/// and the highest of those, at any zoom, against each column worked out on
/// its own.
#[test]
fn each_column_spans_the_step_entering_it_and_its_samples() {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..300 {
        let mut at = 0;
        let samples: Vec<Sample> = (0..random() % 200)
            .map(|_| {
                at += (random() % 500) as i64;
                let mut sample = sample(at, (random() % 100) as i128 - 50);
                if random() % 10 == 0 {
                    sample.cut = Some(at + (random() % 300) as i64);
                }
                if random() % 15 == 0 {
                    sample.value = Number::Float(f64::NAN);
                }
                sample
            })
            .collect();
        let view = View {
            origin: 0,
            start: (random() % 20_000) as f64,
            span: (100 + random() % 100_000) as f64,
            left: 260.0,
            width: (50 + random() % 800) as f64,
        };
        let end = at + (random() % 1000) as i64;
        let drawn = drawn(&samples, end, &view);
        let mut columns: Vec<(f32, f64, f64)> = drawn
            .bars
            .iter()
            .flat_map(|bar| (bar.from as i32..bar.to as i32).map(|x| (x as f32, bar.low, bar.high)))
            .collect();
        let mut expected = Vec::new();
        let right = view.left + view.width as f32;
        let mut x = view.left;
        while x < right {
            let held = samples
                .iter()
                .rev()
                .find(|sample| view.column(sample.cycles) < x)
                .filter(|sample| !sample.value.is_nan())
                .filter(|sample| sample.cut.is_none_or(|cut| view.column(cut) >= x))
                .filter(|_| view.column(end) >= x);
            let inside = samples
                .iter()
                .filter(|sample| view.column(sample.cycles) == x && !sample.value.is_nan());
            let values: Vec<f64> = held
                .into_iter()
                .chain(inside)
                .map(|sample| sample.value.as_f64())
                .collect();
            if !values.is_empty() {
                let low = values.iter().copied().fold(f64::INFINITY, f64::min);
                let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                expected.push((x, low, high));
            }
            x += 1.0;
        }
        columns.retain(|column| column.0 < right);
        assert_eq!(columns, expected, "{samples:?}");
        // Bars that would draw alike are one.
        for pair in drawn.bars.windows(2) {
            assert!(pair[0].to < pair[1].from || (pair[0].low, pair[0].high) != (pair[1].low, pair[1].high));
        }
    }
}

/// A NaN is a gap: nothing until the next sample; zoomed in, every other
/// sample is a dot on its step.
#[test]
fn a_nan_is_a_gap_and_zoomed_in_samples_are_dots() {
    let mut samples = vec![sample(0, 5), sample(10, 0), sample(20, 7)];
    samples[1].value = Number::Float(f64::NAN);
    let drawn = drawn(&samples, 40, &view(40.0, 400.0));
    // The NaN's column shows the step entering it.
    assert_eq!(drawn.bars, [bar(0.0, 101.0, 5.0, 5.0), bar(200.0, 400.0, 7.0, 7.0)]);
    assert_eq!(drawn.dots, [(0.0, 5.0), (200.0, 7.0)]);
}

/// `app::g_position` and `app::g_speed` watched from 100 to 1200 cycles,
/// with gaps from 400 to 500 and from 950 to 1000. The position is written
/// at 150, 300, 600 and 900, the speed at 1100.
fn watched() -> Watched {
    let variable = |name: &str| Curve {
        subject: name.to_owned(),
        field: None,
    };
    let (position, speed) = (variable("app::g_position"), variable("app::g_speed"));
    let sampled = |curve: &Curve, cycles, value| Sampled {
        curve: curve.clone(),
        value: Number::Integer(value),
        at: Edge {
            element: 0,
            cycles,
            lost: false,
        },
    };
    Watched {
        curves: vec![position.clone(), speed.clone()],
        // In the order of the stream, which a curve's own order need not be.
        samples: vec![
            sampled(&position, 300, 2),
            sampled(&position, 150, 1),
            sampled(&position, 600, 3),
            sampled(&position, 900, 4),
            sampled(&speed, 1100, 7),
        ],
        span: Some((100, 1200)),
        gaps: vec![(400, 500), (950, 1000)],
        anchors: 10,
        ..Watched::default()
    }
}

/// A watched variable is a curve named after it, beside the events' own; a
/// gap cuts the step before it, the last step ends with the last anchor, and
/// a loss of the ring cuts no step of it.
#[test]
fn a_watched_variable_is_a_curve_cut_by_its_gaps() {
    let catalogue = catalogue();
    let watched = watched();
    let mut values = Values::new(&catalogue, Some(&watched)).unwrap();
    let names: Vec<String> = values.curves.iter().map(Curve::to_string).collect();
    assert_eq!(
        names,
        ["app::g_position", "app::g_speed", "app::setpoint.f", "app::setpoint.x"]
    );
    let steps = [Step::Setpoint(0, 1, 0.5), Step::Lost, Step::Tick(2000)];
    values.add(&elements(&catalogue, &steps), 0);
    let cuts = |name| -> Vec<(i64, Option<i64>)> {
        values.samples[values.named(name).unwrap()]
            .iter()
            .map(|sample| (sample.cycles, sample.cut))
            .collect()
    };
    assert_eq!(
        cuts("app::g_position"),
        [(150, None), (300, Some(400)), (600, None), (900, Some(950))]
    );
    assert_eq!(cuts("app::g_speed"), [(1100, Some(1200))]);
    assert_eq!(cuts("app::setpoint.x"), [(0, Some(0))]);
    // Unknown in a gap and past the last anchor.
    let position = values.named("app::g_position").unwrap();
    assert!(values.held(position, 350).unwrap().holds_at(350));
    assert!(!values.held(position, 450).unwrap().holds_at(450));
    let speed = values.named("app::g_speed").unwrap();
    assert!(values.held(speed, 1150).unwrap().holds_at(1150));
    assert!(!values.held(speed, 1500).unwrap().holds_at(1500));
}

/// A refused SWO stream adds no curve; an image without event curves still
/// has its watched variables.
#[test]
fn watched_variables_alone_are_values_unless_refused() {
    let mut catalogue = catalogue();
    let refused = Watched {
        refused: Some("no anchor".to_owned()),
        ..watched()
    };
    assert_eq!(Values::new(&catalogue, Some(&refused)).unwrap().curves.len(), 2);
    catalogue.events.clear();
    assert!(Values::new(&catalogue, Some(&refused)).is_none());
    let values = Values::new(&catalogue, Some(&watched())).unwrap();
    assert_eq!(values.samples.iter().map(Vec::len).collect::<Vec<_>>(), [4, 1]);
}

#[test]
fn the_curves_chosen_restore_by_name_those_the_image_has() {
    let catalogue = catalogue();
    let mut values = Values::new(&catalogue, None).unwrap();
    values.restore("app::setpoint.x\nold::curve.y\n");
    assert_eq!(values.saved(), "app::setpoint.x");
    values.restore("app::setpoint.f");
    assert_eq!(values.saved(), "app::setpoint.f\napp::setpoint.x");
}

#[test]
fn edge_values_are_short() {
    assert_eq!(edge(19998.0), "19998");
    assert_eq!(edge(-10000.0), "-10000");
    assert_eq!(edge(0.5), "0.5");
    assert_eq!(edge(1.0 / 3.0), "0.333333");
    assert_eq!(edge(1234.5678), "1234.57");
    assert_eq!(edge(-0.000_012_345_67), "-0.0000123457");
}
