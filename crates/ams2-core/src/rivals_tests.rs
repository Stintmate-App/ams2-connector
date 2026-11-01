//! The rival tracker on real capture frames (Oulton Park
//! practice, 2 Hz, gzip JSON Lines in `fixtures/`) and on a synthetic race.

use crate::codes::{game_state, pit_mode, race_state, session_state};
use crate::messages::ParticipantLap;
use crate::rivals::{FLUSH_INTERVAL_S, RivalTracker};
use crate::testing::RecordingSink;
use crate::tracker::Sink;
use ams2_shm::{SharedMemory, new_frame};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::io::Read;

const FIXTURE: &[u8] = include_bytes!("../fixtures/participants_oulton.jsonl.gz");
const SID: u64 = 1;
const PLAYER: u64 = 0;
const POL: u64 = 1;
const TYLER: u64 = 12;

fn load_frames() -> Vec<Value> {
    let mut text = String::new();
    GzDecoder::new(FIXTURE).read_to_string(&mut text).unwrap();
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn put(dst: &mut [u8], text: &str) {
    dst.fill(0);
    dst[..text.len()].copy_from_slice(text.as_bytes());
}

fn to_shm(frame: &Value, seq: u32) -> Box<SharedMemory> {
    let mut d = new_frame();
    d.version = 14;
    d.sequence_number = seq;
    d.game_state = game_state::INGAME_PLAYING;
    let session = frame["session"].as_str().unwrap();
    d.session_state = session_state::NAMES
        .iter()
        .position(|n| *n == session)
        .unwrap() as u32;
    d.viewed_participant_index = frame["viewed"].as_i64().unwrap() as i32;
    d.num_participants = frame["num"].as_i64().unwrap() as i32;
    for p in frame["participants"].as_array().unwrap() {
        let i = p["i"].as_u64().unwrap() as usize;
        let f = |k: &str| p[k].as_f64().unwrap() as f32;
        let u = |k: &str| p[k].as_u64().unwrap() as u32;
        let info = &mut d.participant_info[i];
        info.is_active = u8::from(p.get("active").and_then(Value::as_bool).unwrap_or(true));
        put(&mut info.name, p["name"].as_str().unwrap());
        info.race_position = u("pos");
        info.laps_completed = u("laps_completed");
        info.current_lap = u("current_lap");
        info.current_lap_distance = f("dist");
        info.current_sector = p["sector"].as_i64().unwrap() as i32;
        put(&mut d.car_names[i], p["car"].as_str().unwrap());
        put(&mut d.car_class_names[i], p["class"].as_str().unwrap());
        d.last_lap_times[i] = f("last_lap");
        d.fastest_lap_times[i] = f("fastest_lap");
        let cur: Vec<f32> = p["cur_s"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        d.current_sector1_times[i] = cur[0];
        d.current_sector2_times[i] = cur[1];
        d.current_sector3_times[i] = cur[2];
        d.laps_invalidated[i] = u8::from(p["lap_invalidated"].as_bool().unwrap());
        d.pit_modes[i] = u("pit_mode");
        d.race_states[i] = u("race_state");
    }
    d
}

fn t_of(frame: &Value) -> f64 {
    frame["t"].as_f64().unwrap()
}

fn replay(frames: &[Value], sink: &mut RecordingSink) {
    let mut tracker = RivalTracker::new();
    for (k, frame) in frames.iter().enumerate() {
        tracker.process(&to_shm(frame, 2 * (k as u32 + 1)), SID, t_of(frame), sink);
    }
    tracker.end_session(t_of(frames.last().unwrap()), sink);
}

fn rows(sink: &RecordingSink, participant: u64) -> Vec<ParticipantLap> {
    let mut r: Vec<_> = sink
        .rival_rows()
        .into_iter()
        .filter(|r| u64::from(r.participant) == participant)
        .collect();
    r.sort_by_key(|r| r.lap);
    r
}

fn part_mut(frame: &mut Value, i: u64) -> &mut Value {
    frame["participants"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["i"].as_u64() == Some(i))
        .unwrap()
}

/// Index of the first frame where participant `i` satisfies `pred`.
fn find(frames: &mut [Value], i: u64, pred: impl Fn(&Value) -> bool) -> usize {
    (0..frames.len())
        .find(|&k| pred(part_mut(&mut frames[k], i)))
        .expect("frame not found")
}

fn pol_crossing(frames: &mut [Value]) -> usize {
    find(frames, POL, |p| p["laps_completed"] == 5)
}

fn close(a: Option<f64>, b: f64, tol: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() <= tol)
}

#[test]
fn test_replay_real_frames() {
    let mut sink = RecordingSink::new();
    replay(&load_frames(), &mut sink);

    let player = rows(&sink, PLAYER);
    // Window starts in sector 3 of the player's lap 3 (sectors not observed), then the pit
    // stop: the pit-lane crossing and the out-lap crossing are not counted by the game.
    assert_eq!(player.iter().map(|r| r.lap).collect::<Vec<_>>(), [3, 4]);
    let (lap3, lap4) = (&player[0], &player[1]);
    assert!(close(lap3.time_s, 142.65, 1e-3));
    assert!(lap3.sectors.is_empty() && !lap3.from_line);
    assert!(lap3.is_player && lap3.pit_stops == Some(0));
    assert!(close(lap4.time_s, 124.912, 1e-3));
    assert_eq!(lap4.time_source.as_deref(), Some("official"));
    assert!(close(lap4.sectors[0], 30.706, 1e-3) && close(lap4.sectors[1], 58.123, 1e-3));
    assert!(close(lap4.sectors[2], 124.912 - 30.706 - 58.123, 1e-3));
    // The counted lap started at the out-lap crossing: no pit mode inside it, one stop before.
    assert!(!lap4.pit && lap4.pit_stops == Some(1) && lap4.from_line);
    assert!(lap4.valid && lap4.position == Some(1));

    let pol = rows(&sink, POL);
    assert_eq!(pol.iter().map(|r| r.lap).collect::<Vec<_>>(), [5]);
    assert!(close(pol[0].time_s, 127.623, 1e-3));
    assert!(close(pol[0].sectors[0], 31.559, 1e-3) && close(pol[0].sectors[1], 59.7, 1e-3));
    assert!(close(pol[0].sectors[2], 36.364, 2e-3));
    assert_eq!(pol[0].pit_stops, Some(1));
    assert!(close(pol[0].sim_best_lap_s, 127.143, 127.143e-4));
    assert_eq!(pol[0].name, "Pol Riera");
    assert_eq!(pol[0].car_id.as_deref(), Some("Ginetta G40 Cup"));
    assert_eq!(pol[0].class_id.as_deref(), Some("G40Cup"));
    assert!(!pol[0].is_player);
    assert_eq!(pol[0].state.as_deref(), Some("RACING")); // AMS2's race state 2, as a label

    assert!(rows(&sink, 18).is_empty()); // the safety car is never tracked
    assert!(rows(&sink, TYLER).len() >= 2);
}

#[test]
fn test_last_lap_time_published_late() {
    // At 50 Hz mLastLapTimes may change a few frames after the lap counter.
    let mut frames = load_frames();
    let k = pol_crossing(&mut frames);
    for f in &mut frames[k..k + 4] {
        part_mut(f, POL)["last_lap"] = 127.284.into(); // still the previous lap's time
    }
    let mut sink = RecordingSink::new();
    replay(&frames, &mut sink);
    let r = rows(&sink, POL);
    assert_eq!(r.len(), 1);
    assert!(close(r[0].time_s, 127.623, 1e-3));
    assert_eq!(r[0].time_source.as_deref(), Some("official"));
    assert!(close(
        r[0].crossed_at_s,
        t_of(&frames[k]) - t_of(&frames[0]),
        1e-3
    ));
}

#[test]
fn test_last_lap_time_never_published_uses_sectors() {
    let mut frames = load_frames();
    let k = pol_crossing(&mut frames);
    for f in &mut frames[k..] {
        part_mut(f, POL)["last_lap"] = 127.284.into();
    }
    let mut sink = RecordingSink::new();
    replay(&frames, &mut sink);
    let r = rows(&sink, POL);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].time_source.as_deref(), Some("sectors"));
    assert!(close(r[0].time_s, 31.559 + 59.7 + 36.365, 2e-3));
}

#[test]
fn test_lap_counter_after_the_sector_index() {
    // The two crossing signals can arrive on different frames: still one lap, timed from the
    // first signal.
    let mut frames = load_frames();
    let k = pol_crossing(&mut frames);
    for f in &mut frames[k..k + 2] {
        let p = part_mut(f, POL);
        p["laps_completed"] = 4.into();
        p["last_lap"] = 127.284.into();
    }
    let mut sink = RecordingSink::new();
    replay(&frames, &mut sink);
    let r = rows(&sink, POL);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].lap, 5);
    assert!(close(r[0].time_s, 127.623, 1e-3));
    assert!(close(
        r[0].crossed_at_s,
        t_of(&frames[k]) - t_of(&frames[0]),
        1e-3
    ));
    assert!(close(r[0].sectors[1], 59.7, 1e-3));
}

#[test]
fn test_invalid_flag() {
    let mut frames = load_frames();
    let k = pol_crossing(&mut frames);
    let start = find(&mut frames, POL, |p| {
        p["laps_completed"] == 4
            && p["pit_mode"] == 0
            && p["sector"] == 0
            && p["lap_invalidated"] == false
            && p["dist"].as_f64().unwrap() < 50.0
    });
    part_mut(&mut frames[start], POL)["lap_invalidated"] = true.into(); // lingering on the crossing frame
    let mut sink = RecordingSink::new();
    replay(&frames, &mut sink);
    assert!(rows(&sink, POL)[0].valid);

    for f in &mut frames[k - 40..k - 30] {
        // mid-lap
        part_mut(f, POL)["lap_invalidated"] = true.into();
    }
    let mut sink2 = RecordingSink::new();
    replay(&frames, &mut sink2);
    assert!(!rows(&sink2, POL)[0].valid);
}

/// Keeps the batches it is handed and the clock at each.
struct Spy {
    now: f64,
    batches: Vec<(f64, usize)>,
}

impl Sink for Spy {
    fn start_session(&mut self, _: &crate::messages::SessionInfo) -> u64 {
        1
    }
    fn end_session(&mut self, _: u64, _: &str) {}
    fn insert_lap(&mut self, _: crate::messages::LapRecord, _: Vec<crate::channels::Row>) {}
    fn insert_setup(&mut self, _: u64, _: &str, _: &str, _: crate::setup::SetupSnapshot) {}
    fn insert_pit_exit(&mut self, _: u64, _: &str, _: Option<f64>) {}
    fn insert_participant_laps(&mut self, session_id: u64, rows: Vec<ParticipantLap>) {
        assert_eq!(session_id, 1);
        self.batches.push((self.now, rows.len()));
    }
}

#[test]
fn test_flush_batches() {
    // Rows are buffered and handed to the sink at most every FLUSH_INTERVAL_S (or when the
    // session tracker flushes them with a player lap / at session end).
    let frames = load_frames();
    let mut spy = Spy {
        now: t_of(&frames[0]),
        batches: Vec::new(),
    };
    let mut tracker = RivalTracker::new();
    for (k, frame) in frames.iter().enumerate() {
        spy.now = t_of(frame);
        tracker.process(&to_shm(frame, 2 * (k as u32 + 1)), 1, spy.now, &mut spy);
    }
    let times: Vec<f64> = spy.batches.iter().map(|b| b.0).collect();
    assert!(times.windows(2).all(|w| w[1] - w[0] > FLUSH_INTERVAL_S));
    tracker.flush(spy.now, &mut spy);
    tracker.flush(spy.now, &mut spy); // nothing left: no empty batch
    tracker.end_session(spy.now, &mut spy);
    assert_eq!(spy.batches.iter().map(|b| b.1).sum::<usize>(), 5); // 2 player + 1 Pol + 2 Tyler
    assert!(spy.batches.iter().all(|b| b.1 > 0));
}

// ------------------------------------------------------- synthetic race

const TRACK: f64 = 3000.0;

/// Player (0) and three AI cars with constant lap times from a standing start. Car 2 drives
/// through the pit lane at the end of lap 2; car 3 is the safety car.
fn race_frames(duration: f64, hz: f64) -> Vec<Box<SharedMemory>> {
    let cars = [
        ("Player One", "MINI Cooper S 1965", "TC60S2", 60.0),
        ("Rival One", "Lotus 23", "TC60S2", 61.0),
        ("Rival Two", "Lotus 23", "TC60S2", 63.0),
        ("Safety Car", "Camaro SafetyCar", "SafetyCar", 999.0),
    ];
    let mut out = Vec::new();
    let mut last = [-123.0f64; 4];
    let mut fastest = [-123.0f64; 4];
    let mut s = [[-1.0f64; 3]; 4];
    for k in 0..=((duration * hz) as usize) {
        let t = k as f64 / hz;
        let mut d = new_frame();
        d.version = 14;
        d.sequence_number = 2 * (k as u32 + 1);
        d.game_state = game_state::INGAME_PLAYING;
        d.session_state = session_state::RACE;
        d.viewed_participant_index = 0;
        d.num_participants = 4;
        let mut progress = [0.0f64; 4];
        for (i, (name, car, cls, lap_t)) in cars.iter().enumerate() {
            let laps = if i != 3 { (t / lap_t).floor() } else { 0.0 };
            let in_lap = t - laps * lap_t;
            let frac = if i != 3 { in_lap / lap_t } else { 0.0 };
            let sector = ((frac * 3.0) as i32).min(2);
            let just_crossed = laps > 0.0 && in_lap < 1.0 / hz;
            if just_crossed {
                last[i] = *lap_t;
                fastest[i] = if fastest[i] < 0.0 {
                    *lap_t
                } else {
                    fastest[i].min(*lap_t)
                };
            }
            // Sectors: final once passed, running in the current one, lingering after it.
            for (j, slot) in s[i].iter_mut().enumerate() {
                if (j as i32) < sector || (just_crossed && j == 2) {
                    *slot = lap_t / 3.0;
                } else if j as i32 == sector {
                    *slot = in_lap - j as f64 * lap_t / 3.0;
                }
            }
            let info = &mut d.participant_info[i];
            info.is_active = 1;
            put(&mut info.name, name);
            info.laps_completed = laps as u32;
            info.current_lap = laps as u32 + 1;
            info.current_lap_distance = (frac * TRACK) as f32;
            info.current_sector = sector;
            put(&mut d.car_names[i], car);
            put(&mut d.car_class_names[i], cls);
            d.last_lap_times[i] = last[i] as f32;
            d.fastest_lap_times[i] = fastest[i] as f32;
            d.current_sector1_times[i] = s[i][0] as f32;
            d.current_sector2_times[i] = s[i][1] as f32;
            d.current_sector3_times[i] = s[i][2] as f32;
            let mut pit = pit_mode::NONE;
            let dt = t - 2.0 * cars[2].3;
            if i == 2 && 1.0 < dt && dt < 20.0 {
                pit = if 5.0 < dt && dt < 10.0 {
                    pit_mode::IN_PIT
                } else {
                    pit_mode::DRIVING_OUT_OF_PITS
                };
            }
            if i == 2 && -5.0 < dt && dt < 1.0 {
                pit = pit_mode::DRIVING_INTO_PITS;
            }
            if i == 3 {
                pit = pit_mode::IN_PIT;
            }
            d.pit_modes[i] = pit;
            d.race_states[i] = race_state::RACING;
            progress[i] = if i != 3 { laps + frac } else { -1.0 };
        }
        let mut order: Vec<usize> = (0..4).collect();
        order.sort_by(|a, b| progress[*b].total_cmp(&progress[*a]));
        for (pos, i) in order.into_iter().enumerate() {
            d.participant_info[i].race_position = pos as u32 + 1;
        }
        out.push(d);
    }
    out
}

#[test]
fn test_race_rows() {
    let mut sink = RecordingSink::new();
    let mut tracker = RivalTracker::new();
    let mut now = 0.0;
    for d in race_frames(200.0, 10.0) {
        now += 0.1;
        tracker.process(&d, SID, now, &mut sink);
    }
    tracker.end_session(now, &mut sink);
    let all = sink.rival_rows();
    let mut who: Vec<u32> = all.iter().map(|r| r.participant).collect();
    who.sort_unstable();
    who.dedup();
    assert_eq!(who, [0, 1, 2]); // never the safety car
    let one = rows(&sink, 1);
    assert_eq!(one.iter().map(|r| r.lap).collect::<Vec<_>>(), [1, 2, 3]);
    // The same last lap time twice is not a new one: the later laps fall back to the sectors.
    let sources: Vec<_> = one
        .iter()
        .map(|r| r.time_source.as_deref().unwrap())
        .collect();
    assert_eq!(sources, ["official", "sectors", "sectors"]);
    assert!(one.iter().all(|r| (r.time_s.unwrap() - 61.0).abs() < 1e-3));
    assert!(
        close(one[0].sectors[0], 61.0 / 3.0, 1e-3) && close(one[0].sectors[1], 61.0 / 3.0, 1e-3)
    );
    let two = rows(&sink, 2);
    assert_eq!(
        two.iter().map(|r| r.pit).take(3).collect::<Vec<_>>(),
        [false, true, true]
    ); // through the pit lane
    assert!(all.iter().all(|r| r.state.as_deref() == Some("RACING")));
}
