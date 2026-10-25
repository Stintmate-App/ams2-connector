//! `dump`: per-tick state of the player over a time range of a capture, for investigations.

use crate::sampler::Sampler;
use ams2_core::announce::detector::EventDetector;
use ams2_core::testing::{FakeClock, RecordingSink};
use ams2_core::tracker::SessionTracker;
use std::io::BufReader;
use std::path::Path;

/// Prints one line per processed tick in `[from_s, to_s]` (capture seconds): the viewed
/// participant's lap, distance, sector, `mLapInvalidated`, speed, flag colour and reason, the
/// game / session / race states, the pit mode, the tracker's live lap decision and the
/// announcements the detector emitted. `step` keeps one tick in `step`.
pub fn run(capture: &Path, from_s: f64, to_s: f64, step: u64) -> Result<(), String> {
    let file = std::fs::File::open(capture).map_err(|e| format!("{}: {e}", capture.display()))?;
    let Some(mut sampler) =
        Sampler::new(BufReader::with_capacity(1 << 20, file)).map_err(|e| e.to_string())?
    else {
        return Err("the capture holds no record".into());
    };
    let clock = FakeClock::new();
    let mut tracker = SessionTracker::new(Box::new(clock.clone()));
    let mut detector = EventDetector::new(clock.clone(), 2.0);
    let mut sink = RecordingSink::new();
    let t0_us = sampler.t0_us();
    let mut n = 0u64;
    while let Some(rel_us) = sampler.advance().map_err(|e| e.to_string())? {
        let t = rel_us as f64 / 1e6;
        if t > to_s {
            break;
        }
        clock.set((t0_us + rel_us) as f64 / 1e6);
        let d = sampler.frame();
        tracker.process(d, &mut sink);
        let ann = detector.update(d);
        let calls = std::mem::take(&mut sink.calls);
        n += 1;
        if t < from_s || (!n.is_multiple_of(step) && ann.is_empty() && calls.is_empty()) {
            continue;
        }
        let v = usize::try_from(d.viewed_participant_index)
            .unwrap_or(0)
            .min(63);
        let p = &d.participant_info[v];
        let live = tracker.live_lap();
        println!(
            "t={t:8.2} v={v} lap={} dist={:7.1} sec={} inv={} lapsInv={} spd={:5.1} flag={} reason={} flags[v]={} game={} sess={} race={} pit={} lapT={:.2} | live={:?} | ann={:?} | calls={}",
            p.current_lap,
            p.current_lap_distance,
            p.current_sector,
            d.lap_invalidated,
            d.laps_invalidated[v],
            d.speed,
            d.highest_flag_colour,
            d.highest_flag_reason,
            d.highest_flag_colours[v],
            d.game_state,
            d.session_state,
            d.race_state,
            d.pit_mode,
            d.current_time,
            live.map(|l| (l.lap_type, l.invalid_cause.is_some(), l.invalid_at_m)),
            ann.iter().map(|a| a.code).collect::<Vec<_>>(),
            calls.len(),
        );
    }
    Ok(())
}
