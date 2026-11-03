//! Ableton Link: tempo and beat phase shared with other applications on the
//! local network (Live, Traktor, Resolume, phones…).
//!
//! Built on the official Link library through `rusty_link` (cargo feature
//! `link`, on by default).

use rusty_link::{AblLink, SessionState};

/// Beats per bar for phase alignment.
const QUANTUM: f64 = 4.0;

/// A Link session member. Joins the session when created, leaves when
/// dropped.
pub struct Link {
    link: AblLink,
    state: SessionState,
}

impl Link {
    pub fn new(bpm: f64) -> Self {
        let link = AblLink::new(bpm);
        link.enable(true);
        Self {
            link,
            state: SessionState::new(),
        }
    }

    /// Other applications in the session.
    pub fn peers(&self) -> u64 {
        self.link.num_peers()
    }

    /// Session tempo and the beat position now, phase-aligned to the bar.
    pub fn capture(&mut self) -> (f64, f64) {
        self.link.capture_app_session_state(&mut self.state);
        let now = self.link.clock_micros();
        (self.state.tempo(), self.state.beat_at_time(now, QUANTUM))
    }

    /// Changes the tempo of the whole session.
    pub fn set_tempo(&mut self, bpm: f64) {
        self.link.capture_app_session_state(&mut self.state);
        let now = self.link.clock_micros();
        self.state.set_tempo(bpm, now);
        self.link.commit_app_session_state(&self.state);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.link.enable(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn beats_advance_at_the_tempo() {
        let mut link = Link::new(120.0);
        let (bpm, start) = link.capture();
        assert_eq!(bpm, 120.0);
        std::thread::sleep(Duration::from_millis(500));
        let (_, later) = link.capture();
        // Half a second at 120 BPM is one beat.
        assert!((later - start - 1.0).abs() < 0.1, "{start} → {later}");
        link.set_tempo(90.0);
        assert!((link.capture().0 - 90.0).abs() < 1e-6);
    }

    #[test]
    fn two_peers_share_the_tempo() {
        let mut a = Link::new(120.0);
        let mut b = Link::new(100.0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while a.peers() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if a.peers() == 0 {
            eprintln!("Link peers cannot see each other here (no multicast), skipping");
            return;
        }
        a.set_tempo(133.0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while (b.capture().0 - 133.0).abs() > 0.01 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!((b.capture().0 - 133.0).abs() < 0.01);
    }
}
