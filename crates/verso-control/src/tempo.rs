//! Tap tempo.

use std::time::{Duration, Instant};

/// Gap after which taps start a new measurement.
const RESET: Duration = Duration::from_secs(2);
/// Intervals averaged.
const HISTORY: usize = 8;

#[derive(Debug, Default)]
pub struct TapTempo {
    taps: Vec<Instant>,
}

impl TapTempo {
    /// Registers a tap at `now`. Returns the tempo once two taps are close
    /// enough together.
    pub fn tap(&mut self, now: Instant) -> Option<f32> {
        if self
            .taps
            .last()
            .is_some_and(|&last| now.saturating_duration_since(last) > RESET)
        {
            self.taps.clear();
        }
        self.taps.push(now);
        if self.taps.len() > HISTORY + 1 {
            self.taps.remove(0);
        }
        let (first, last) = (self.taps.first()?, self.taps.last()?);
        let intervals = self.taps.len() - 1;
        if intervals == 0 {
            return None;
        }
        let average = last.duration_since(*first).as_secs_f32() / intervals as f32;
        (average > 0.0).then(|| 60.0 / average)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_intervals_and_resets_after_a_pause() {
        let mut tap = TapTempo::default();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        assert_eq!(tap.tap(ms(0)), None);
        assert_eq!(tap.tap(ms(500)), Some(120.0));
        let bpm = tap.tap(ms(1010)).unwrap();
        assert!((bpm - 118.8).abs() < 0.1, "{bpm}");
        // After a long pause, a single tap starts over.
        assert_eq!(tap.tap(ms(5000)), None);
        assert_eq!(tap.tap(ms(5400)), Some(150.0));
    }
}
