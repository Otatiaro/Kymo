use super::*;

const HZ: f64 = 250e6;

/// A target at `HZ` whose counter reads `start` at time 0, writing an event at
/// each of `times` (ns, in order), drained by passes every `period` ns that
/// read the write index and settle what the pass before read, as `kymo
/// record` does. Returns each event's cycles from the timeline, and the
/// cycles its counter really read.
fn record(timeline: &mut Timeline, start: u64, times: &[f64], period: f64) -> (Vec<i64>, Vec<i64>) {
    let truth: Vec<i64> = times
        .iter()
        .map(|time| (start as f64 + time * HZ / 1e9) as i64)
        .collect();
    let mut found = Vec::new();
    let mut read_end = 0;
    let mut pass = 0.0;
    while found.len() < times.len() {
        pass += period;
        let write_index = times.iter().take_while(|time| **time < pass).count();
        timeline.pass(1_700_000_000_000_000_000 + pass as u64, write_index as u32);
        while found.len() < read_end {
            let index = found.len();
            found.push(timeline.event(index as u32, truth[index] as u32));
        }
        read_end = write_index;
    }
    (found, truth)
}

/// The offsets of `cycles` from the first.
fn elapsed(cycles: &[i64]) -> Vec<i64> {
    cycles.iter().map(|each| each - cycles[0]).collect()
}

/// Events every millisecond from `from` to `to` seconds.
fn every_millisecond(from: f64, to: f64) -> impl Iterator<Item = f64> {
    (0..((to - from) * 1e3) as usize).map(move |n| (from + n as f64 * 1e-3) * 1e9)
}

#[test]
fn signed_differences_without_passes() {
    let mut timeline = Timeline::new(None);
    let cycles: Vec<i64> = [0xffff_fff0u32, 0x10, 0x0c, 0x8000_0000]
        .iter()
        .enumerate()
        .map(|(index, timestamp)| timeline.event(index as u32, *timestamp))
        .collect();
    assert_eq!(cycles, [0xffff_fff0, 0x1_0000_0010, 0x1_0000_000c, 0x1_8000_0000]);
    assert_eq!(timeline.frequency(), None);
    assert_eq!(timeline.seconds(cycles[1], Some(32.0)), Some(1.0));
}

#[test]
fn turns_from_host_time_at_the_given_clock() {
    let mut timeline = Timeline::new(Some(HZ as u64));
    // 30 s idle is 1.75 turns at 250 MHz, 100 s is 5.8; the first event comes
    // before any pass.
    let times = [0.0, 30e9, 30.001e9, 130e9, 130.0005e9];
    let (found, truth) = record(&mut timeline, 0xffff_0000, &times, 1e6);
    assert_eq!(elapsed(&found), elapsed(&truth));
    let seconds = timeline.seconds(found[3], timeline.frequency()).unwrap();
    assert!((seconds - 130.0).abs() < 1e-6, "{seconds}");
}

#[test]
fn turns_from_the_measured_frequency() {
    let mut timeline = Timeline::new(None);
    let times: Vec<f64> = every_millisecond(0.0, 2.0)
        .chain(every_millisecond(42.0, 42.5))
        .chain([100e9])
        .collect();
    let (found, truth) = record(&mut timeline, 12345, &times, 1e6);
    assert_eq!(elapsed(&found), elapsed(&truth));
    let (hz, span) = timeline.measured().unwrap();
    assert!((hz / HZ - 1.0).abs() < 1e-4, "{hz}");
    assert!((span - 100.0).abs() < 1e-2, "{span}");
}

#[test]
fn measured_once_the_events_span_a_second() {
    let mut timeline = Timeline::new(None);
    let times: Vec<f64> = every_millisecond(0.0, 0.9).collect();
    record(&mut timeline, 0, &times, 1e6);
    assert_eq!(timeline.measured(), None);
    let mut timeline = Timeline::new(None);
    let times: Vec<f64> = every_millisecond(0.0, 1.1).collect();
    record(&mut timeline, 0, &times, 1e6);
    assert!(timeline.measured().is_some());
}

#[test]
fn an_event_is_halfway_between_the_passes_around_it() {
    let mut timeline = Timeline::new(None);
    // Events 0 and 1 were written before the first pass.
    for (host_ns, write_index) in [(0, 2), (10, 2), (20, 2), (30, 3), (40, 3), (50, 5)] {
        timeline.pass(1000 + host_ns, write_index);
    }
    let hosts: Vec<_> = (0..5).map(|index| timeline.host(index)).collect();
    assert_eq!(hosts, [Some(0.0), Some(0.0), Some(25.0), Some(45.0), Some(45.0)]);
}

#[test]
fn idle_passes_are_not_kept() {
    let mut timeline = Timeline::new(None);
    for pass in 0..10_000 {
        timeline.pass(pass * 1_000_000, 7);
    }
    assert_eq!(timeline.passes.len(), 2);
}

#[test]
fn a_counter_that_does_not_count_has_no_frequency() {
    // QEMU has no cycle counter: every timestamp reads 0.
    let mut timeline = Timeline::new(None);
    let times: Vec<f64> = every_millisecond(0.0, 3.0).collect();
    for (index, _) in times.iter().enumerate() {
        timeline.pass(index as u64 * 1_000_000, index as u32 + 1);
        assert_eq!(timeline.event(index as u32, 0), 0);
    }
    assert_eq!(timeline.frequency(), None);
}
