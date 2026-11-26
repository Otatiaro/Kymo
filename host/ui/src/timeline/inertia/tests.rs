use super::*;

/// Where a glide from `velocity` is after `seconds`, by frames of `dt`, and
/// its speed then.
fn stepped(velocity: f64, seconds: f64, dt: f64) -> (f64, f64) {
    let (mut at, mut velocity) = (0.0, velocity);
    for _ in 0..(seconds / dt).round() as usize {
        let (moved, slower) = glided(velocity, dt);
        at += moved;
        velocity = slower;
    }
    (at, velocity)
}

/// The view moves by the integral of an exponentially decaying speed: the
/// same at 30, 60 and 144 frames a second, `v₀·τ` pixels in all.
#[test]
fn a_glide_follows_the_continuous_law_at_any_frame_rate() {
    for velocity in [-4000.0, 250.0, 3000.0] {
        let seconds = 1.0;
        let law = velocity * TAU * (1.0 - (-seconds / TAU).exp());
        for rate in [30.0, 60.0, 144.0] {
            let (at, speed) = stepped(velocity, seconds, 1.0 / rate);
            assert!((at - law).abs() < 1e-6, "{velocity} at {rate}: {at} against {law}");
            assert!((speed - velocity * (-seconds / TAU).exp()).abs() < 1e-6);
        }
        let (all, _) = stepped(velocity, 30.0, 1.0 / 60.0);
        assert!((all - velocity * TAU).abs() < 1e-6, "{all}");
    }
}

/// A view 1 000 pixels wide over 1 000 cycles, in the middle of a capture of
/// a million.
fn timeline() -> Timeline {
    let ticks: Vec<_> = (0..=1000).map(|n| Some(n * 1000)).collect();
    let mut timeline = super::super::jump::tests::timeline(&ticks);
    timeline.span = 1000.0;
    timeline.start = 500_000.0;
    timeline
}

const WIDTH: f64 = 1000.0;
const FRAME: f64 = 1.0 / 60.0;

/// Let go at 2 000 pixels a second leftward, the view moves later in the
/// capture, by the law, then stops below `STOP`; let go slower, it stays.
#[test]
fn a_view_let_go_glides_then_stops() {
    let mut timeline = timeline();
    timeline.release(-2000.0);
    let mut frames = 0;
    while timeline.glide(FRAME, WIDTH, false) {
        frames += 1;
        assert!(frames < 1000, "never stops");
    }
    // Stopped once slower than STOP: v₀·e^(−t/τ) < STOP.
    let seconds = frames as f64 * FRAME;
    assert!(2000.0 * (-seconds / TAU).exp() >= STOP * 0.9, "{seconds}");
    let moved = timeline.start - 500_000.0;
    let law = 2000.0 * TAU * (1.0 - (-(seconds + FRAME) / TAU).exp());
    assert!((moved - law).abs() < 1e-6, "{moved} against {law}");
    assert!(timeline.glide.is_none());

    let mut timeline = self::timeline();
    timeline.release(STOP / 2.0);
    assert!(!timeline.glide(FRAME, WIDTH, false));
    assert_eq!(timeline.start, 500_000.0);
}

/// A press on the view, or the view moved by anything else, stops a glide.
#[test]
fn a_press_or_another_move_stops_a_glide() {
    let mut timeline = timeline();
    timeline.release(2000.0);
    assert!(timeline.glide(FRAME, WIDTH, false));
    let start = timeline.start;
    assert!(!timeline.glide(FRAME, WIDTH, true));
    assert_eq!(timeline.start, start);
    assert!(!timeline.glide(FRAME, WIDTH, false), "stays stopped");

    for moved in [|t: &mut Timeline| t.start += 10.0, |t: &mut Timeline| t.span *= 2.0] {
        let mut timeline = self::timeline();
        timeline.release(2000.0);
        assert!(timeline.glide(FRAME, WIDTH, false));
        moved(&mut timeline);
        let view = (timeline.start, timeline.span);
        assert!(!timeline.glide(FRAME, WIDTH, false));
        assert_eq!((timeline.start, timeline.span), view);
    }
}

/// A glide stops at an end of the capture, the view within its bounds.
#[test]
fn a_glide_stops_at_an_end_of_the_capture() {
    for velocity in [4000.0_f64, -4000.0] {
        let mut timeline = timeline();
        let bound = if velocity > 0.0 {
            timeline.first()
        } else {
            timeline.last() - timeline.span / 2.0
        };
        // 1 000 cycles, so 1 000 pixels, from the end: less than v₀·τ.
        timeline.start = bound + velocity.signum() * 1000.0;
        timeline.release(velocity);
        let mut frames = 0;
        while timeline.glide(FRAME, WIDTH, false) {
            frames += 1;
        }
        assert_eq!(timeline.start, bound, "{velocity}");
        assert!(frames < 60, "{velocity}: {frames} frames, stopped early by the end");
    }
}
