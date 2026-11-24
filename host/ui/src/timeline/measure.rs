use super::{NARROWEST, Timeline};
use eframe::egui;
use std::time::Instant;

/// `--measure`: frames drawn without vsync, the whole capture then the
/// narrowest view at its middle, after a warm-up each.
#[derive(Default)]
pub(super) struct Measure {
    frame: u32,
    last: Option<Instant>,
    /// Per view: each frame's interval, its time on the CPU as eframe
    /// counts it (all but the wait to present), and the time to lay the
    /// timeline out, in milliseconds.
    samples: [Vec<[f64; 3]>; 2],
}

const WARM_UP: u32 = 30;
const FRAMES: u32 = 300;

impl Timeline {
    pub(super) fn measure_frame(&mut self, ctx: &egui::Context, cpu: f32) {
        let painted = self.painted;
        let Some(measure) = self.measure.as_mut() else {
            return;
        };
        let now = Instant::now();
        let phase = WARM_UP + FRAMES;
        if let Some(last) = measure.last {
            let frame = measure.frame - 1;
            if frame % phase >= WARM_UP {
                measure.samples[(frame / phase) as usize].push([
                    (now - last).as_secs_f64() * 1e3,
                    f64::from(cpu) * 1e3,
                    painted,
                ]);
            }
        }
        measure.last = Some(now);
        let frame = measure.frame;
        measure.frame += 1;
        ctx.request_repaint();
        if frame == 0 {
            self.whole();
        } else if frame == phase {
            let middle = (self.first() + self.last()) / 2.0;
            self.span = NARROWEST;
            self.start = middle - NARROWEST / 2.0;
        } else if frame == 2 * phase {
            for (name, samples) in ["whole capture", "narrowest view"].iter().zip(&measure.samples) {
                let column = |n: usize| {
                    let mut sorted: Vec<f64> = samples.iter().map(|sample| sample[n]).collect();
                    sorted.sort_by(f64::total_cmp);
                    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
                    format!("median {:.2} p99 {:.2} max {:.2}", at(0.5), at(0.99), at(1.0))
                };
                println!(
                    "{name} ({} frames, ms): frame {}; cpu {}; timeline {}",
                    samples.len(),
                    column(0),
                    column(1),
                    column(2)
                );
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
