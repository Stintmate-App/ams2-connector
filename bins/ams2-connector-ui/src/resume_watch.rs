//! Watches a kept-open session while no recorder runs.
//!
//! A recorder stopped while the game runs leaves `session-resume.json` (see the recorder's
//! `sink` module) so the next one continues the same session. If the game goes to its menus or
//! another session begins before that, the file must go, otherwise a later session of the same
//! kind could join the old one. The recorder's own continuity check
//! (`SessionClock::continues_as`) stands alone; this watcher adds the one thing it cannot see
//! afterwards, a stay in the menus between two sessions with equal clocks. It reads the game
//! about once a second (a dozen fields from the mapping, no frame copy), only while the file
//! exists and no recorder runs.

use ams2_shm::{SessionClock, Verdict};
use std::path::Path;

/// The file the recorder leaves, in the connector's data folder.
pub const FILE: &str = "session-resume.json";

/// What one look at the game found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    /// The game is not running.
    Gone,
    /// No usable reading now (the game was writing, or no mapping yet): look again.
    Torn,
    /// The game's clock.
    Clock(SessionClock),
}

/// What to do with the stored session given a reading; `None`: keep it and look again.
pub fn decide(stored: &SessionClock, reading: &Reading) -> Option<Verdict> {
    match reading {
        Reading::Gone => Some(Verdict::Drop),
        Reading::Torn => None,
        Reading::Clock(now) => Some(stored.watch(now)),
    }
}

/// The resume file exists.
pub fn pending(path: &Path) -> bool {
    path.is_file()
}

/// Applies [`decide`] to the file at `path` and deletes it on `Drop`; true when deleted. A file
/// that cannot be read as a stored clock is left to the recorder, which consumes it.
pub fn check(path: &Path, reading: &Reading) -> bool {
    let Some(stored) = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| SessionClock::from_json(v.get("clock")?))
    else {
        return false;
    };
    if decide(&stored, reading) == Some(Verdict::Drop) {
        let removed = std::fs::remove_file(path).is_ok();
        if removed {
            tracing::info!("Kept-open session forgotten: the game left it ({reading:?})");
        }
        return removed;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored() -> SessionClock {
        SessionClock {
            game_state: 2,
            session_state: 1,
            identity: 9,
            time_remaining_s: 1200.0,
            laps_in_event: 0,
            duration_min: 30.0,
            laps_completed: 4,
        }
    }

    fn file(dir: &Path) -> std::path::PathBuf {
        let p = dir.join(FILE);
        let v = serde_json::json!({ "key": "k", "clock": stored().to_json() });
        std::fs::write(&p, v.to_string()).unwrap();
        p
    }

    #[test]
    fn the_decision_on_synthetic_states() {
        let s = stored();
        let at = |f: &dyn Fn(&mut SessionClock)| {
            let mut c = s;
            f(&mut c);
            decide(&s, &Reading::Clock(c))
        };
        assert_eq!(
            at(&|c| c.game_state = 4),
            Some(Verdict::Keep),
            "garage menu"
        );
        assert_eq!(at(&|c| c.time_remaining_s = 800.0), Some(Verdict::Keep));
        assert_eq!(at(&|c| c.game_state = 1), Some(Verdict::Drop), "front end");
        assert_eq!(at(&|c| c.game_state = 0), Some(Verdict::Drop), "exited");
        assert_eq!(
            at(&|c| c.session_state = 3),
            Some(Verdict::Drop),
            "session type"
        );
        assert_eq!(
            at(&|c| c.identity = 10),
            Some(Verdict::Drop),
            "car or track"
        );
        let restarted = |c: &mut SessionClock| {
            c.time_remaining_s = 1800.0;
            c.laps_completed = 0;
        };
        assert_eq!(at(&restarted), Some(Verdict::Drop), "a new session");
        assert_eq!(decide(&s, &Reading::Gone), Some(Verdict::Drop));
        assert_eq!(decide(&s, &Reading::Torn), None);
    }

    #[test]
    fn the_file_goes_only_on_a_drop() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(dir.path());
        let mut ok = stored();
        ok.game_state = 4;
        assert!(!check(&p, &Reading::Clock(ok)) && pending(&p));
        assert!(!check(&p, &Reading::Torn) && pending(&p));
        let mut menus = stored();
        menus.game_state = 1;
        assert!(check(&p, &Reading::Clock(menus)));
        assert!(!pending(&p));
    }

    #[test]
    fn an_unreadable_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE);
        std::fs::write(&p, "{}").unwrap();
        assert!(!check(&p, &Reading::Gone) && pending(&p));
    }
}
