//! The view let go while it moves (ADR-100): released during a drag, it goes
//! on at the pointer's speed and slows down by friction, until it is slower
//! than `STOP` or meets an end of the capture.
//!
//! The speed is in pixels per second, so a glide looks the same at any zoom.
//! It decays exponentially with time, `v(t) = v₀·e^(−t/τ)`, and the view moves
//! by its integral, whatever the frames' lengths: the glide is the same at
//! any frame rate, and covers `v₀·τ` pixels in all.

use super::Timeline;

/// The friction's time constant τ, in seconds.
pub(super) const TAU: f64 = 0.325;
/// The slowest glide, in pixels per second: let go slower, the view stays.
pub(super) const STOP: f64 = 30.0;

/// From a speed of `velocity` pixels per second, after `dt` seconds: the
/// pixels moved and the speed then.
pub(super) fn glided(velocity: f64, dt: f64) -> (f64, f64) {
    let decay = (-dt / TAU).exp();
    (velocity * TAU * (1.0 - decay), velocity * decay)
}

/// A glide: its speed, and the view it left at the last frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Glide {
    pub(super) velocity: f64,
    start: f64,
    span: f64,
}

impl Timeline {
    /// Lets the view go at `velocity` pixels per second, rightward positive.
    pub(super) fn release(&mut self, velocity: f64) {
        self.glide = (velocity.abs() >= STOP).then_some(Glide {
            velocity,
            start: self.start,
            span: self.span,
        });
    }

    /// Moves the view of `width` pixels by `dt` seconds of its glide; stops
    /// it when `pressed`, when anything else moved the view since the last
    /// frame, at an end of the capture, or once it is slower than `STOP`.
    /// Whether it still glides.
    pub(super) fn glide(&mut self, dt: f64, width: f64, pressed: bool) -> bool {
        let Some(glide) = self.glide.take() else {
            return false;
        };
        if pressed || glide.start != self.start || glide.span != self.span {
            return false;
        }
        let (moved, velocity) = glided(glide.velocity, dt);
        let start = self.start - moved * self.span / width;
        self.start = start;
        self.clamp();
        if self.start != start || velocity.abs() < STOP {
            return false;
        }
        self.glide = Some(Glide {
            velocity,
            start: self.start,
            span: self.span,
        });
        true
    }
}

#[cfg(test)]
mod tests;
