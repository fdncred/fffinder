//! `--term --fps`: how many frames a second reach the terminal, in the status
//! bar.

use super::Finder;
use gpui_kit::{Context, Global};
use std::time::{Duration, Instant};

/// Set as a global when the status bar should show frames per second.
pub struct ShowFps;

impl Global for ShowFps {}

/// Turns the running count of frames shown into frames per second, sampled
/// about once a second.
#[derive(Debug, Clone, PartialEq)]
pub struct FpsMeter {
    /// The frame count at the last sample, and when it was taken.
    last: (u64, Instant),
    /// The rate shown, once the first sample is in.
    fps: Option<u32>,
    /// Whether the last sample changed the rate shown. Showing it took a
    /// frame, which the next sample leaves out, so an idle window reads a
    /// steady 0 instead of flipping between 0 and 1.
    redrew: bool,
}

impl FpsMeter {
    /// Start counting from `frames`, the count at `now`.
    pub fn new(frames: u64, now: Instant) -> FpsMeter {
        FpsMeter {
            last: (frames, now),
            fps: None,
            redrew: false,
        }
    }

    /// The rate to show; `None` until the first sample.
    pub fn fps(&self) -> Option<u32> {
        self.fps
    }

    /// Take a sample of the running frame count.
    ///
    /// # Arguments
    ///
    /// * `frames` - Frames shown since the app started
    /// * `now` - When the count was read
    ///
    /// # Returns
    ///
    /// True when the rate shown changed, so the status bar needs drawing.
    pub fn sample(&mut self, frames: u64, now: Instant) -> bool {
        let (then_frames, then) = self.last;
        let elapsed = now.saturating_duration_since(then);
        if elapsed.is_zero() {
            return false;
        }
        self.last = (frames, now);
        let drawn = frames
            .saturating_sub(then_frames)
            .saturating_sub(u64::from(self.redrew));
        // `as` from a float rounds toward zero and saturates; `round` first
        // makes it the nearest whole number.
        let fps = (drawn as f64 / elapsed.as_secs_f64()).round() as u32;
        let changed = self.fps != Some(fps);
        self.fps = Some(fps);
        self.redrew = changed;
        changed
    }
}

impl Finder {
    /// Show frames per second in the status bar, updated every second.
    pub(super) fn start_fps_meter(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        self.fps = Some(FpsMeter::new(crate::term::frames_shown(), now));
        self.fps_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let updated = this.update(cx, |this, cx| {
                    let now = cx.background_executor().now();
                    if let Some(meter) = &mut this.fps
                        && meter.sample(crate::term::frames_shown(), now)
                    {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(start: Instant, secs: f64) -> Instant {
        start + Duration::from_secs_f64(secs)
    }

    #[test]
    fn counts_frames_over_the_time_that_passed() {
        let start = Instant::now();
        let mut meter = FpsMeter::new(100, start);
        assert_eq!(meter.fps(), None);

        assert!(meter.sample(160, secs(start, 1.)));
        assert_eq!(meter.fps(), Some(60));

        // A late timer: 1.5 seconds of frames still read as a per-second rate.
        // The redraw for the last change is left out: 89 / 1.5 rounds to 59.
        assert!(meter.sample(250, secs(start, 2.5)));
        assert_eq!(meter.fps(), Some(59));
    }

    #[test]
    fn an_idle_window_reads_a_steady_zero() {
        let start = Instant::now();
        let mut meter = FpsMeter::new(0, start);

        assert!(meter.sample(0, secs(start, 1.)), "the first rate is shown");
        assert_eq!(meter.fps(), Some(0));
        // Showing it drew one frame, which isn't counted.
        assert!(!meter.sample(1, secs(start, 2.)));
        assert_eq!(meter.fps(), Some(0));
        assert!(!meter.sample(1, secs(start, 3.)));
        assert_eq!(meter.fps(), Some(0));
    }

    #[test]
    fn ignores_a_sample_with_no_time_passed() {
        let start = Instant::now();
        let mut meter = FpsMeter::new(5, start);
        assert!(!meter.sample(9, start));
        assert_eq!(meter.fps(), None);
        assert!(meter.sample(9, secs(start, 1.)));
        assert_eq!(meter.fps(), Some(4));
    }
}
