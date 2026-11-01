//! Plays a raw capture or the scripted session into a test mapping with the real seqlock
//! writer, so the recorder (or `diagnose`) can read it by name as it would read the game.
//! Refuses the game's own mapping name: it can never write to the game.

use ams2_core::testing::scripted_session;
use ams2_platform::timer::WaitableTimer;
use ams2_shm::capture::CaptureReader;
use ams2_shm::testing::TestMapping;
use ams2_shm::{SharedMemory, ShmError, new_frame};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// One frame to write and, for captures, the microsecond it was recorded at.
pub type Timed = (Option<u64>, Box<SharedMemory>);

/// The scripted session (no timestamps: played at a fixed rate).
pub fn scripted() -> Vec<Timed> {
    scripted_session()
        .into_iter()
        .map(|f| (None, f.frame))
        .collect()
}

/// Every frame of a raw capture file with its timestamp.
pub fn capture(path: &Path) -> Result<Vec<Timed>, ShmError> {
    let mut reader = CaptureReader::new(std::io::BufReader::new(std::fs::File::open(path)?))?;
    let mut out = Vec::new();
    loop {
        let mut f = new_frame();
        match reader.next_frame(&mut f)? {
            Some(us) => out.push((Some(us), f)),
            None => {
                if reader.truncated() {
                    eprintln!(
                        "warning: {}: the last record is cut short; kept {} whole frames",
                        path.display(),
                        out.len()
                    );
                }
                return Ok(out);
            }
        }
    }
}

/// Writes `frames` into `map`: timestamped frames at their recorded times divided by `speed`,
/// the others every `1 / hz` s divided by `speed`. Returns the frames written.
pub fn play(
    map: &mut TestMapping,
    frames: &[Timed],
    hz: f64,
    speed: f64,
    stop: &AtomicBool,
) -> u64 {
    let timer = WaitableTimer::new().ok();
    let start = std::time::Instant::now();
    let mut written = 0;
    for (i, (us, f)) in frames.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let at = us.map_or(i as f64 / hz, |us| us as f64 / 1e6) / speed;
        let wait = at - start.elapsed().as_secs_f64();
        if wait > 0.0 {
            let d = Duration::from_secs_f64(wait);
            match &timer {
                Some(t) => {
                    let _ = t.sleep(d);
                }
                None => std::thread::sleep(d),
            }
        }
        map.write_frame(f);
        written += 1;
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use ams2_shm::testing::unique_name;
    use ams2_shm::{FrameSource, Mapping, SnapshotOutcome};

    #[test]
    fn the_scripted_session_reads_back_through_the_real_seqlock() {
        let name = unique_name("replay");
        let mut map = TestMapping::create(&name).unwrap();
        let reader = Mapping::open(&name).unwrap();
        let frames = scripted();
        assert!(frames.len() > 100);
        let stop = AtomicBool::new(false);
        let n = play(&mut map, &frames, 50.0, 1e6, &stop);
        assert_eq!(n as usize, frames.len());
        let mut out = new_frame();
        let mut reader = reader;
        let SnapshotOutcome::Frame(seq) = reader.snapshot(&mut out).unwrap() else {
            panic!("a consistent frame");
        };
        assert_eq!(seq, map.sequence());
        assert_eq!(out.version, frames.last().unwrap().1.version);
    }

    #[test]
    fn the_games_own_name_is_refused() {
        assert!(TestMapping::create(ams2_shm::MAPPING_NAME).is_err());
    }

    #[test]
    fn a_capture_round_trips_with_its_timestamps() {
        use ams2_shm::capture::CaptureWriter;
        let dir = std::env::temp_dir().join(format!("ams2-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c.bin");
        let mut w = CaptureWriter::new(std::fs::File::create(&file).unwrap()).unwrap();
        for (i, (_, f)) in scripted().iter().take(3).enumerate() {
            w.write_frame(1000 * i as u64, f).unwrap();
        }
        w.finish().unwrap();
        let got = capture(&file).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            got.iter().map(|g| g.0).collect::<Vec<_>>(),
            [Some(0), Some(1000), Some(2000)]
        );
    }
}
