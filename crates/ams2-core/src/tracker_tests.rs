//! Port of `tests/test_tracker.py`: the session / lap state machine on synthetic frames, with
//! a recording sink instead of the Writer (stints are Pitwall's; the sink, the outbox and the
//! f32 encoding on the wire are tested with the sink in task B1).

use crate::channels::{CHANNEL_NAMES, LAP_CHANNEL_NAMES, N_CHANNELS, PIT, T};
use crate::codes::{game_state as gs, pit_mode as pm, session_state as ss};
use crate::messages::{lap_message, session_start_message};
use crate::names::Names;
use crate::testing::{Call, Harness, StoredLap};
use crate::tracker::{Event, LiveLap, drop_aborted_sectors};
use ams2_shm::SharedMemory;

fn near(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() <= 1e-4 + b.abs() * 1e-6)
}

fn f(v: f64) -> f32 {
    v as f32
}

fn laps(h: &Harness) -> Vec<StoredLap> {
    h.sink.laps()
}

fn summary(h: &Harness) -> Vec<(u32, &'static str, Option<f64>)> {
    laps(h)
        .iter()
        .map(|l| (l.lap, l.kind, l.record.lap_time_s))
        .collect()
}

fn session_ended(h: &Harness) -> Vec<(u64, u32, bool)> {
    h.sink
        .events()
        .iter()
        .filter_map(|e| match e {
            Event::SessionEnded {
                session_id,
                laps,
                discarded,
            } => Some((*session_id, *laps, *discarded)),
            _ => None,
        })
        .collect()
}

fn lap_events(h: &Harness) -> Vec<crate::tracker::LapEvent> {
    h.sink
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Event::LapCompleted(l) => Some(l.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn test_full_flow() {
    let mut h = Harness::new();
    // Join mid-lap 1 (out lap from the garage), then cross the line into lap 2.
    h.sim.d.pit_mode = pm::NONE;
    h.drive(1, 40.0, 90.0, 2000.0);
    // Lap 2: sectors appear during the lap; official time is published one frame after crossing.
    h.drive(2, 0.0, 30.0, 0.0);
    h.sim.d.current_sector1_time = 30.5;
    h.drive(2, 30.0, 60.0, 1300.0);
    h.sim.d.current_sector2_time = 29.0;
    h.drive(2, 60.0, 91.0, 2700.0);
    h.feed(3, 1.0, 0.02); // crossed the line, time not published yet
    assert_eq!(laps(&h).len(), 1); // only lap 1
    h.sim.d.last_lap_time = f(90.512);
    h.feed(3, 2.0, 0.04);
    // Lap 3 gets invalidated, then the driver quits to the menu mid-lap.
    h.sim.d.lap_invalidated = 1;
    h.drive(3, 1.0, 20.0, 0.0);
    h.sim.d.game_state = gs::FRONT_END;
    h.feed(3, 900.0, 20.0);

    let stored = laps(&h);
    assert_eq!(stored.iter().map(|l| l.lap).collect::<Vec<_>>(), [1, 2, 3]);
    let (lap1, lap2, lap3) = (&stored[0], &stored[1], &stored[2]);
    assert!(lap1.record.lap_time_s.is_none() && lap1.kind == "out"); // partial out lap, no time
    assert!(near(lap2.record.lap_time_s, 90.512));
    assert_eq!(lap2.record.lap_time_source.as_deref(), Some("official"));
    assert!(near(lap2.record.sector1_s, 30.5) && near(lap2.record.sector2_s, 29.0));
    assert!(near(lap2.record.sector3_s, 90.512 - 30.5 - 29.0));
    assert!(lap2.record.valid && lap2.kind == "flying");
    assert!(lap3.record.lap_time_s.is_none() && !lap3.record.valid); // partial and invalid
    for l in &stored {
        assert_eq!(lap_message(&l.record, &l.rows).sectors.len(), 3);
    }

    // The session as the write API gets it.
    let crate::testing::Call::Start(info) = &h.sink.calls[0] else {
        panic!("first call")
    };
    assert_eq!(
        crate::vocab::session_type_of_name(&info.session_state),
        "practice"
    );
    assert_eq!(
        (
            info.car.as_str(),
            info.car_class.as_str(),
            info.track.as_str(),
            info.variation.as_str(),
            info.length_m
        ),
        (
            "MINI Cooper S 1965",
            "TC60S",
            "Road_Atlanta",
            "Full",
            Some(4088.0)
        )
    );
    assert_eq!(info.fuel_capacity_l, Some(51.0));
    let (catalog, _) = session_start_message(info, Names::embedded());
    assert_eq!(
        catalog.expect("catalog").cars[0].fuel_capacity_l,
        Some(51.0)
    );
    let kinds = h.sink.kinds();
    assert_eq!(kinds[..3], ["catalog", "session_start", "setup"]);
    assert_eq!(*kinds.last().unwrap(), "session_end");
    assert_eq!(kinds.iter().filter(|k| **k == "lap").count(), 3);
}

#[test]
fn test_channels_roundtrip_and_tyre_orientation() {
    let mut h = Harness::new();
    h.sim.d.terrain = [46, 40, 0, 7]; // run-off, kerb B1, road, grass
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_GARAGE;
    h.drive(1, 0.0, 10.0, 0.0);
    h.end();

    let stored = laps(&h);
    assert_eq!(stored.len(), 1);
    let msg = lap_message(&stored[0].record, &stored[0].rows);
    let ch = msg.channels.expect("channels");
    let names: Vec<&str> = ch.0.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, LAP_CHANNEL_NAMES);
    assert_eq!(names.len(), CHANNEL_NAMES.len());
    assert!(!names.iter().any(|n| n.ends_with("_terrain")));
    let col = |n: &str| &ch.0.iter().find(|(c, _)| *c == n).unwrap().1;
    assert!((col("speed")[0] - 100.0).abs() < 0.01);
    // Left wheels: game Right = inner. Right wheels: game Left = inner.
    assert_eq!(col("fl_t_in")[0], 60.0);
    assert_eq!(col("fl_t_out")[0], 50.0);
    assert_eq!(col("fr_t_in")[0], 60.0);
    assert_eq!(col("fr_t_out")[0], 50.0);
    // Pitwall's codes, not AMS2's: surfaces and pit mode (driving out of the garage -> 3).
    let surfaces: Vec<f32> = ["fl", "fr", "rl", "rr"]
        .iter()
        .map(|w| col(&format!("{w}_surface"))[0])
        .collect();
    assert_eq!(surfaces, [5.0, 1.0, 0.0, 2.0]);
    assert!(col("pit_mode").iter().all(|v| *v == 3.0));
    // The lap message carries the compact float32 encoding.
    let v = serde_json::to_value(&ch).unwrap();
    assert_eq!(v["speed"]["encoding"], "f32");
}

#[test]
fn test_official_time_timeout_falls_back_to_measured() {
    let mut h = Harness::new();
    h.feed(1, 1.0, 0.0);
    h.drive(1, 0.0, 80.0, 0.0);
    h.feed(2, 1.0, 0.02);
    h.tick(5.0);
    h.feed(2, 50.0, 1.0);
    let stored = laps(&h);
    assert_eq!(stored.len(), 1);
    let r = &stored[0].record;
    assert!(near(r.lap_time_s, 79.0) && near(r.measured_time_s, 79.0));
    assert_eq!(r.lap_time_source.as_deref(), Some("measured"));
}

#[test]
fn test_pause_keeps_lap_open_and_setup_snapshot() {
    let mut h = Harness::new();
    h.drive(1, 0.0, 10.0, 0.0);
    h.sim.d.game_state = gs::INGAME_PAUSED;
    h.feed(1, 450.0, 10.0);
    h.sim.d.game_state = gs::INGAME_PLAYING;
    h.drive(1, 10.0, 20.0, 450.0);
    assert!(h.tracker.session.is_some() && h.tracker.lap.is_some());
    assert_eq!(h.tracker.lap.as_ref().unwrap().rows.len(), 20);

    let setups = h.sink.setups();
    assert_eq!(setups.len(), 1);
    let (reason, snap) = setups[0];
    assert_eq!(reason, "session_start");
    let msg = crate::setup::setup_message(snap, "ISO", reason);
    assert_eq!(msg.source, "telemetry");
    assert_eq!(
        (msg.brake_bias_front_pct, msg.fuel_l),
        (Some(80.0), Some(25.5))
    );
    // Carcasses at ambient (cold tyres): the pressures are the cold setting.
    assert_eq!(
        (msg.pressure_fl_kpa, msg.pressure_rr_kpa),
        (Some(174.0), Some(192.0))
    );
    assert!(msg.abs == Some(0.0) && msg.wing_front.is_none()); // mAntiLockSetting 0; no wings
    assert_eq!(h.sink.stored_sessions(), 0); // no lap yet: the Writer stores nothing
}

#[test]
fn test_end_session_stores_the_last_lap_and_the_session_end() {
    // Port of test_sink_flushes_on_close: what the tracker hands over when the recorder stops.
    let mut h = Harness::new();
    h.drive(1, 0.0, 90.0, 0.0);
    h.sim.d.last_lap_time = f(90.0);
    h.feed(2, 1.0, 0.02);
    h.drive(2, 0.0, 10.0, 0.0);
    h.end();
    let got: Vec<_> = laps(&h)
        .iter()
        .map(|l| (l.lap, l.record.lap_time_s))
        .collect();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, 1);
    assert!(near(got[0].1, 90.0));
    assert_eq!(got[1], (2, None));
    assert!(matches!(
        h.sink
            .calls
            .iter()
            .rev()
            .find(|c| matches!(c, Call::End { .. })),
        Some(Call::End { .. })
    ));
}

#[test]
fn test_partial_lap_kept_when_game_clock_resets_in_pits() {
    let mut h = Harness::new();
    h.drive(1, 0.0, 30.0, 0.0);
    // Back in the garage the game's lap clock goes to -1.
    h.sim.d.pit_mode = pm::IN_GARAGE;
    h.feed_kmh(1, 0.0, -1.0, 0.0);
    h.tick(1.0);
    h.sim.d.game_state = gs::FRONT_END;
    h.feed_kmh(1, 0.0, -1.0, 0.0);
    assert_eq!(summary(&h), [(1, "in", None)]);
}

#[test]
fn test_stints_split_on_pit_lane_stop() {
    let mut h = Harness::new();
    // Session starts in the garage, then the car drives out (still stint 1).
    h.sim.d.pit_mode = pm::IN_GARAGE;
    h.feed_kmh(1, 3900.0, -1.0, 0.0);
    h.tick(1.0);
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_GARAGE;
    h.drive(1, 0.0, 10.0, 3900.0);
    h.sim.d.pit_mode = pm::NONE;
    h.drive(1, 10.0, 30.0, 4000.0);
    // Lap 2: flying lap.
    h.drive(2, 0.0, 90.0, 0.0);
    h.sim.d.last_lap_time = f(90.0);
    h.feed(3, 1.0, 0.02);
    // Lap 3: half a lap, then into the pits, stop, and out again (setup change).
    h.drive(3, 0.04, 40.0, 0.0);
    h.sim.d.pit_mode = pm::DRIVING_INTO_PITS;
    h.drive(3, 40.0, 50.0, 1800.0);
    h.sim.d.pit_mode = pm::IN_PIT;
    h.drive_kmh(3, 50.0, 60.0, 2000.0, 0.0);
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_PITS;
    h.drive(3, 60.0, 70.0, 2000.0);
    h.sim.d.pit_mode = pm::NONE;
    h.drive(3, 70.0, 100.0, 2300.0);
    h.sim.d.last_lap_time = f(100.0);
    h.feed(4, 1.0, 0.02);
    h.drive(4, 0.04, 88.0, 0.0);
    h.sim.d.last_lap_time = f(88.0);
    h.feed(5, 1.0, 0.02);
    h.end();

    let got = summary(&h);
    let want = [
        (1, "out", None), // out lap from the garage
        (2, "flying", Some(90.0)),
        (3, "in", None),  // in-lap, closed at pit exit
        (4, "out", None), // out lap of stint 2 (no official time: started in the pits)
        (5, "flying", Some(88.0)),
    ];
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(want) {
        assert_eq!((g.0, g.1), (w.0, w.1));
        assert_eq!(g.2.is_some(), w.2.is_some());
        if let Some(t) = w.2 {
            assert!(near(g.2, t));
        }
    }
    assert_eq!(
        laps(&h).iter().map(|l| l.sim_lap).collect::<Vec<_>>(),
        [1, 2, 3, 3, 4]
    );
    let reasons: Vec<&str> = h.sink.setups().iter().map(|(r, _)| *r).collect();
    assert_eq!(reasons, ["session_start", "pit_exit", "pit_exit"]);
    // On the wire: on_track on every lap, and each pit_exit after the laps that ended before it
    // and before the setup taken at it.
    let order: Vec<&str> = h
        .sink
        .kinds()
        .into_iter()
        .filter(|k| ["lap", "setup", "pit_exit"].contains(k))
        .collect();
    assert_eq!(
        order,
        [
            "setup", "pit_exit", "setup", "lap", "lap", "lap", "pit_exit", "setup", "lap", "lap"
        ]
    );
    assert!(
        laps(&h)
            .iter()
            .all(|l| lap_message(&l.record, &l.rows).on_track == Some(true))
    );
    for c in &h.sink.calls {
        if let Call::PitExit { fuel_l, .. } = c {
            assert!(fuel_l.unwrap() > 0.0);
        }
    }
}

#[test]
fn test_return_to_garage_via_menu_with_lap_reset_keeps_session() {
    let mut h = Harness::new();
    h.feed(1, 1.0, 0.0);
    h.drive(1, 0.0, 90.0, 0.0);
    h.sim.d.last_lap_time = f(90.0);
    h.feed(2, 1.0, 0.02);
    h.drive(2, 0.04, 30.0, 0.0);
    // Driver stops and uses the menu: teleported to the garage, the game resets the lap count.
    h.sim.d.game_state = gs::INGAME_INMENU_TIME_TICKING;
    h.sim.d.pit_mode = pm::IN_GARAGE;
    h.feed_kmh(2, 1300.0, 30.0, 0.0);
    h.sim.d.game_state = gs::INGAME_PLAYING;
    for _ in 0..3 {
        h.tick(1.0);
        h.feed_kmh(1, 0.0, -1.0, 0.0);
    }
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_GARAGE;
    h.drive(1, 0.0, 20.0, 3900.0);
    h.sim.d.pit_mode = pm::NONE;
    h.drive(1, 20.0, 40.0, 4000.0);
    h.end();

    assert_eq!(h.sink.stored_sessions(), 1);
    let stored = summary(&h);
    assert_eq!((stored[0].0, stored[0].1), (1, "flying"));
    assert!(near(stored[0].2, 90.0));
    assert_eq!(stored[1].0, 2);
    assert!(stored[1].2.is_none()); // partial lap before the menu
    // Pitwall numbers the stints: one pit_exit follows the return to the garage.
    let exits = h
        .sink
        .calls
        .iter()
        .filter(|c| matches!(c, Call::PitExit { .. }))
        .count();
    assert_eq!(exits, 1);
}

/// Real AMS2 sequence (sessions 1-4): garage -> pit exit -> track at ~378 m, with the game lap
/// clock at -1 and mCurrentLap = 1 until well after the first crossing.
fn leave_garage(h: &mut Harness) {
    h.sim.d.pit_mode = pm::IN_GARAGE;
    h.sim.d.fuel_level = f(27.0 / 51.0);
    h.feed_kmh(1, 92.0, -1.0, 0.0);
    h.tick(1.0);
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_GARAGE;
    h.drive(1, 0.0, 5.0, 92.0);
    h.sim.d.pit_mode = pm::NONE;
    // The rest of the out lap: the game lap clock stays at -1 (lap not timed).
    h.sim.d.fuel_level = f(26.0 / 51.0);
    for dist in (400..4088).step_by(100) {
        h.feed(1, f64::from(dist), -1.0);
        h.tick(1.0);
    }
}

#[test]
fn test_first_crossing_after_garage_without_lap_counter_change() {
    let mut h = Harness::new();
    leave_garage(&mut h);
    // First crossing: distance wraps, timing starts, but mCurrentLap stays at 1.
    h.drive(1, 0.03, 30.0, 0.0);
    h.sim.d.current_sector1_time = f(44.9);
    h.drive(1, 30.0, 60.0, 1300.0);
    h.sim.d.current_sector2_time = f(46.3);
    h.drive(1, 60.0, 116.5, 2700.0);
    // Second crossing: the counter increments now; the official time arrives a frame later.
    h.feed(2, 1.0, 0.02);
    h.tick(0.02);
    h.sim.d.last_lap_time = f(116.54);
    h.feed(2, 2.0, 0.04);
    h.drive(2, 1.0, 20.0, 40.0);
    h.end();

    let stored = laps(&h);
    let got: Vec<_> = stored.iter().map(|l| (l.lap, l.sim_lap, l.kind)).collect();
    assert_eq!(got, [(1, 1, "out"), (2, 1, "flying"), (3, 2, "flying")]);
    assert!(stored[0].record.lap_time_s.is_none() && stored[2].record.lap_time_s.is_none());
    assert!(near(stored[1].record.lap_time_s, 116.54));
    assert_eq!(
        stored[1].record.lap_time_source.as_deref(),
        Some("official")
    );
    assert!(near(stored[1].record.sector1_s, 44.9) && near(stored[1].record.sector2_s, 46.3));
}

fn one_crossing_two_signals(counter_first: bool) {
    let mut h = Harness::new();
    h.feed(1, 1.0, 0.0);
    h.drive(1, 0.0, 90.0, 0.0);
    if counter_first {
        // counter increments while the distance is still at the end
        h.feed(2, 4087.9, 90.0);
        h.tick(0.02);
        h.sim.d.last_lap_time = f(90.0);
        h.feed(2, 1.0, 0.02);
    } else {
        // distance wraps first, counter and official time on the next frame
        h.feed(1, 1.0, 0.02);
        h.tick(0.02);
        h.sim.d.last_lap_time = f(90.0);
        h.feed(2, 2.0, 0.04);
    }
    h.drive(2, 1.0, 88.0, 40.0);
    // Lap 2 ends but the game publishes no new time: lap 2 must not inherit 90.0.
    h.feed(3, 1.0, 0.02);
    h.tick(5.0);
    h.feed(3, 60.0, 1.0);
    h.end();

    let stored = laps(&h);
    assert_eq!(stored[0].lap, 1);
    assert!(near(stored[0].record.lap_time_s, 90.0));
    assert_eq!(
        stored[0].record.lap_time_source.as_deref(),
        Some("official")
    );
    assert_eq!(stored[1].lap, 2);
    assert!(
        stored[1]
            .record
            .lap_time_s
            .is_some_and(|t| (t - 87.0).abs() < 0.1)
    );
    assert_eq!(
        stored[1].record.lap_time_source.as_deref(),
        Some("measured")
    );
    let rest: Vec<_> = stored[2..]
        .iter()
        .map(|l| (l.lap, l.record.lap_time_s))
        .collect();
    assert_eq!(rest, [(3, None)]); // partial, no time
}

#[test]
fn test_one_crossing_two_signals_splits_once_and_keeps_times_apart() {
    one_crossing_two_signals(false);
    one_crossing_two_signals(true);
}

#[test]
fn test_fuel_and_sectors_ignore_garage_reset_and_stale_values() {
    let mut h = Harness::new();
    h.feed(1, 1.0, 0.0);
    h.sim.d.fuel_level = f(20.0 / 51.0);
    h.drive(1, 0.0, 30.0, 0.0);
    h.sim.d.current_sector1_time = 45.0;
    h.drive(1, 30.0, 60.0, 1300.0);
    h.sim.d.current_sector2_time = f(46.7);
    h.drive(1, 60.0, 90.0, 2700.0);
    h.sim.d.last_lap_time = f(90.0);
    h.feed(2, 1.0, 0.02);
    // Lap 2: S1/S2 still hold lap 1's values; the driver goes to the garage via the menu at
    // 26 s. On that frame the game writes an aborted S1 and resets the car (full tank).
    h.sim.d.fuel_level = f(17.0 / 51.0);
    h.drive(2, 0.04, 26.0, 0.0);
    h.sim.d.pit_mode = pm::IN_GARAGE;
    h.sim.d.current_sector1_time = f(26.06);
    h.sim.d.fuel_level = f(27.0 / 51.0);
    h.feed_kmh(2, 92.0, -1.0, 0.0);
    h.end();

    let stored = laps(&h);
    let lap2 = &stored[1];
    assert_eq!(lap2.lap, 2);
    let r = &lap2.record;
    assert!(r.fuel_start_l.is_some_and(|v| (v - 20.0).abs() < 0.01));
    // Used = start - the last sample on track (17 L), not the 27 L reset.
    assert!(
        lap_message(r, &lap2.rows)
            .fuel_used_l
            .is_some_and(|v| (v - 3.0).abs() < 0.01)
    );
    assert_eq!((r.sector1_s, r.sector2_s, r.sector3_s), (None, None, None)); // neither the aborted S1 nor lap 1's S2
    assert_eq!(lap2.kind, "in");
}

#[test]
fn test_duplicate_sequence_is_ignored() {
    let mut h = Harness::new();
    h.sim.frame(1, 10.0, 1.0);
    h.tracker.process(&h.sim.d, &mut h.sink);
    h.tracker.process(&h.sim.d, &mut h.sink);
    assert_eq!(h.tracker.lap.as_ref().unwrap().rows.len(), 1);
}

#[test]
fn test_drop_aborted_sectors() {
    let rows = |last_t: f64| {
        [1.0, last_t].map(|t| {
            let mut r = [0.0; N_CHANNELS];
            r[T] = t;
            r[PIT] = 0.0;
            r
        })
    };
    // Out of fuel at 99.4 s: S1 is real, S2 is the running time when the session ended.
    assert_eq!(
        drop_aborted_sectors(Some(45.587), Some(53.829), &rows(99.416)),
        (Some(45.587), None)
    );
    // Teleported to the garage at 26 s: the "S1" is the running time.
    assert_eq!(
        drop_aborted_sectors(Some(26.057), None, &rows(26.04)),
        (None, None)
    );
    // In-lap that went into the pits after completing S1 and S2.
    assert_eq!(
        drop_aborted_sectors(Some(46.181), Some(46.841), &rows(117.6)),
        (Some(46.181), Some(46.841))
    );
}

#[test]
fn test_race_load_formation_and_start_stay_in_one_session() {
    // Real sequence of the 2026-10-07 race: the race loads still reporting the qualy's lap
    // counter (4), the counter drops to 1 for the formation lap (lap clock -1, distance stuck
    // at 0), then timing starts on track and lap 1 runs to the line.
    let mut h = Harness::new();
    h.sim.d.session_state = ss::RACE;
    h.sim.d.ambient_temperature = f(11.7);
    h.sim.d.tyre_carcass_temp = [f(273.15 + 45.0); 4]; // pre-heated tyres at the race start
    for _ in 0..10 {
        // grid, stale counter
        h.feed_kmh(4, 0.0, -1.0, 0.0);
        h.tick(1.0);
    }
    h.sim.d.fuel_level = f(31.0 / 51.0);
    for i in 0..26 {
        // formation: counter 1, timing not started, distance stuck at 0
        h.feed_kmh(1, 0.0, -1.0, 50.0 + f64::from(i));
        h.tick(1.0);
    }
    h.sim.d.fuel_level = f(30.5 / 51.0);
    h.drive(1, 0.0, 125.0, 0.0);
    h.sim.d.last_lap_time = f(125.38);
    h.feed(2, 1.0, 0.02);
    h.drive(2, 1.0, 20.0, 40.0);
    h.end();

    assert_eq!(h.sink.stored_sessions(), 1);
    let Call::Start(info) = &h.sink.calls[0] else {
        panic!()
    };
    assert_eq!(
        crate::vocab::session_type_of_name(&info.session_state),
        "race"
    );
    let stored = laps(&h);
    let lap1 = &stored[stored.len() - 2]; // race lap 1, from the timing start
    assert_eq!(lap1.kind, "flying");
    assert!(near(lap1.record.lap_time_s, 125.38));
    assert_eq!(lap1.record.lap_time_source.as_deref(), Some("official"));
    let pre_race = &stored[..stored.len() - 2];
    assert!(!pre_race.is_empty());
    assert!(
        pre_race
            .iter()
            .all(|l| l.kind == "formation" && l.record.lap_time_s.is_none())
    );
    // Pre-heated tyres: the pressures read are not the cold setting, so they are left out.
    let setups = h.sink.setups();
    let msg = crate::setup::setup_message(setups[0].1, "ISO", "session_start");
    assert_eq!(
        (msg.pressure_fl_kpa, msg.brake_bias_front_pct),
        (None, Some(80.0))
    );
    let kinds: Vec<&str> = lap_events(&h).iter().map(|e| e.kind).collect();
    assert!(kinds[..pre_race.len()].iter().all(|k| *k == "formation"));
}

#[test]
fn test_race_lap_waited_in_the_pit_box_is_not_a_formation_lap() {
    // formation lap, lap 1 timed, an in lap, a stay in the box with no lap
    // clock, an out lap. The box lap is part of the stop (`in`), never `formation`.
    let mut h = Harness::new();
    h.sim.d.session_state = ss::RACE;
    for i in 0..20 {
        h.feed_kmh(1, 0.0, -1.0, 50.0 + f64::from(i)); // formation lap: no lap clock
        h.tick(1.0);
    }
    assert_eq!(live(&h).lap_type, "formation"); // before the start it is still formation
    h.drive(1, 0.0, 90.0, 0.0); // lap 1, timed
    assert_eq!(live(&h).lap_type, "flying");
    h.sim.d.last_lap_time = f(90.0);
    h.feed(2, 1.0, 0.02);
    h.drive(2, 1.0, 60.0, 40.0);
    h.sim.d.pit_mode = pm::DRIVING_INTO_PITS; // in lap
    h.drive(2, 60.0, 80.0, 2000.0);
    assert_eq!(live(&h).lap_type, "in");
    // The counter rolls in the pit lane (the new lap opens there, no lap clock), then the box.
    h.feed_kmh(3, 5.0, -1.0, 40.0);
    h.tick(1.0);
    h.sim.d.pit_mode = pm::IN_PIT;
    for i in 0..40 {
        h.feed_kmh(3, 100.0, -1.0, 20.0 + f64::from(i % 3));
        h.tick(1.0);
    }
    assert_eq!(
        live(&h).lap_type,
        "in",
        "a lap waited in the box is not formation"
    );
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_PITS; // out lap
    for i in 0..40 {
        h.feed_kmh(3, 200.0 + f64::from(i) * 10.0, -1.0, 60.0);
        h.tick(1.0);
    }
    assert_eq!(live(&h).lap_type, "out");
    h.end();

    let kinds: Vec<&str> = laps(&h).iter().map(|l| l.kind).collect();
    assert!(!kinds.is_empty() && kinds[0] == "formation", "{kinds:?}");
    assert_eq!(
        kinds.iter().filter(|k| **k == "formation").count(),
        1,
        "only the lap before the start is formation: {kinds:?}"
    );
}

#[test]
fn test_empty_session_is_never_stored() {
    let mut h = Harness::new();
    h.feed_kmh(1, 0.0, -1.0, 0.0); // a session that never got going
    h.tick(2.0);
    h.sim.d.game_state = gs::FRONT_END;
    h.feed_kmh(1, 0.0, -1.0, 0.0);
    assert_eq!(h.sink.stored_sessions(), 0);
    assert_eq!(session_ended(&h), [(1, 0, true)]);
    // The connector simply ended it; the Writer does not store it.
    assert_eq!(
        h.sink.kinds(),
        ["catalog", "session_start", "setup", "session_end"]
    );
}

#[test]
fn test_lap_events_and_weather() {
    let mut h = Harness::new();
    h.sim.d.track_temperature = 22.0;
    h.sim.d.cloud_brightness = 0.4;
    h.sim.d.fuel_level = f(30.0 / 51.0);
    h.feed(1, 1.0, 0.0);
    h.drive(1, 0.0, 90.0, 0.0);
    h.sim.d.last_lap_time = f(90.0);
    h.sim.d.fuel_level = f(27.0 / 51.0);
    h.feed(2, 1.0, 0.02);
    h.drive(2, 0.04, 45.0, 0.0);
    h.sim.d.fuel_level = f(24.0 / 51.0);
    h.drive(2, 45.0, 89.0, 2000.0);
    h.sim.d.last_lap_time = f(89.5);
    h.feed(3, 1.0, 0.02);
    h.end();

    let started = h.sink.events().into_iter().find_map(|e| match e {
        Event::SessionStarted {
            session_type,
            track,
            ..
        } => Some((session_type.clone(), track.clone())),
        _ => None,
    });
    assert_eq!(started, Some(("PRACTICE".into(), "Road_Atlanta".into())));
    let evs = lap_events(&h);
    let (lap1, lap2) = (&evs[0], &evs[1]);
    assert_eq!((lap1.lap, lap2.lap), (1, 2)); // the stored lap numbers
    assert_eq!(lap1.lap_time_s, Some(90.0));
    assert!(lap1.session_best);
    assert_eq!(lap1.delta_session_best_s, None);
    assert_eq!(lap2.lap_time_s, Some(89.5));
    assert_eq!(lap2.delta_session_best_s, Some(-0.5));
    assert!(lap2.session_best && lap2.fuel_used_l.is_some_and(|v| (v - 3.0).abs() < 0.01));
    assert!(lap2.fuel_laps_left.is_some_and(|v| (v - 8.0).abs() < 0.1)); // 24 L / 3 L per lap
    assert_eq!(lap2.track_c, Some(22.0));
    let first = &laps(&h)[0];
    // Conditions at the end of the lap; AMS2's cloud brightness is not a cover: not carried.
    let m = lap_message(&first.record, &first.rows);
    let c = m.conditions.unwrap();
    assert!(near(c.track_c, 22.0) && c.cloud.is_none());
    assert!(matches!(
        h.sink.events().last(),
        Some(Event::SessionEnded { .. })
    ));
}

#[allow(clippy::too_many_arguments)]
fn rival_frame(
    h: &mut Harness,
    lap: u32,
    dist: f64,
    t: f64,
    lc: u32,
    sector: i32,
    last: f64,
    position: u32,
    last_lap_time: Option<f64>,
) {
    let d = &mut h.sim.d;
    let p = &mut d.participant_info[0];
    p.laps_completed = lc;
    p.current_sector = sector;
    p.race_position = position;
    d.last_lap_times[0] = f(last);
    d.current_sector1_times[0] = if sector >= 1 { 30.0 } else { f(t) };
    // S2 lingers into the next lap until its sector 2 starts (as in the game).
    d.current_sector2_times[0] = if sector >= 2 || lc >= 1 {
        29.0
    } else {
        f((t - 30.0).max(0.0))
    };
    d.participant_info[0].name[..10].copy_from_slice(b"Player One");
    d.car_names[0][..18].copy_from_slice(b"MINI Cooper S 1965");
    d.car_class_names[0][..5].copy_from_slice(b"TC60S");
    if let Some(l) = last_lap_time {
        d.last_lap_time = f(l);
    }
    h.feed(lap, dist, t);
}

#[test]
fn test_rival_rows_flushed_with_player_laps_and_position_event() {
    let mut h = Harness::new();
    let mut t = 0.0;
    while t < 90.0 {
        // lap 1 from the line, sectors at 30 / 60 s
        let sector = if t < 30.0 {
            0
        } else if t < 60.0 {
            1
        } else {
            2
        };
        rival_frame(&mut h, 1, t / 90.0 * 4088.0, t, 0, sector, -123.0, 3, None);
        h.tick(1.0);
        t += 1.0;
    }
    rival_frame(&mut h, 2, 1.0, 0.02, 1, 0, 90.5, 2, Some(90.5));
    h.tick(1.0);
    rival_frame(&mut h, 2, 20.0, 1.0, 1, 0, 90.5, 2, None);

    // The player's lap was saved, so the rival row went to the sink with it.
    let rows = h.sink.rival_rows();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(
        (
            r.name.as_str(),
            r.lap,
            r.time_s,
            r.is_player,
            r.position,
            r.valid,
            r.pit
        ),
        ("Player One", 1, Some(90.5), true, Some(2), true, false)
    );
    assert_eq!(r.car_id.as_deref(), Some("MINI Cooper S 1965"));
    let s: Vec<f64> = r.sectors.iter().map(|s| s.unwrap()).collect();
    assert!((s[0] - 30.0).abs() < 1e-4 && (s[1] - 29.0).abs() < 1e-4 && (s[2] - 31.5).abs() < 1e-4);
    let ev = lap_events(&h);
    let last = ev.last().unwrap();
    assert_eq!((last.lap_time_s, last.position), (Some(90.5), Some(2)));
    // The batch went out before the lap's own write has no rival row yet: order in the sink.
    assert!(h.sink.kinds().contains(&"participant_laps"));
}

#[test]
fn test_discarded_session_drops_rival_rows() {
    // The Writer drops the rows of a session with no lap; here, the tracker's side: the session
    // is ended with no lap, so nothing is stored for it.
    let mut h = Harness::new();
    rival_frame(&mut h, 1, 0.0, -1.0, 0, 0, -123.0, 1, None);
    h.sim.d.game_state = gs::FRONT_END;
    rival_frame(&mut h, 1, 0.0, -1.0, 0, 0, -123.0, 1, None);
    assert_eq!(h.sink.stored_sessions(), 0);
    assert_eq!(session_ended(&h), [(1, 0, true)]);
    assert!(h.sink.laps().is_empty());
}

// ---- a lap ended by "return to garage" from the menu is an in lap ----------------

/// Session 28, lap_id 166: a timed lap to the line, then ~35 s of the next lap, then the driver
/// opens the menu (game paused / in menu) and chooses "return to garage".
fn line_then_menu(h: &mut Harness) {
    h.feed(1, 1.0, 0.0);
    h.drive(1, 0.0, 90.0, 0.0);
    h.sim.d.last_lap_time = f(90.0);
    h.feed(2, 1.0, 0.02);
    h.drive(2, 0.04, 35.0, 0.0);
    // In the menu the car is still on track first...
    h.sim.d.game_state = gs::INGAME_INMENU_TIME_TICKING;
    h.feed_kmh(2, 1500.0, 35.0, 0.0);
    h.tick(1.0);
    // ...then "return to garage": the pit mode shows the garage, the game still in the menu.
    h.sim.d.pit_mode = pm::IN_GARAGE;
    h.feed_kmh(2, 1500.0, 35.0, 0.0);
    h.tick(1.0);
}

#[test]
fn test_bug013_return_to_garage_from_menu_then_quit_is_an_in_lap() {
    let mut h = Harness::new();
    line_then_menu(&mut h);
    h.sim.d.game_state = gs::FRONT_END;
    h.feed_kmh(2, 0.0, -1.0, 0.0);
    let got = summary(&h);
    assert_eq!(got.len(), 2);
    assert_eq!((got[0].0, got[0].1), (1, "flying"));
    assert!(near(got[0].2, 90.0));
    assert_eq!(got[1], (2, "in", None));
}

#[test]
fn test_bug013_return_to_garage_then_drive_again() {
    let mut h = Harness::new();
    line_then_menu(&mut h);
    h.sim.d.game_state = gs::INGAME_PLAYING;
    // Back in the garage, the game resets the lap counter; then the car drives out.
    for _ in 0..3 {
        h.feed_kmh(1, 0.0, -1.0, 0.0);
        h.tick(1.0);
    }
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_GARAGE;
    h.drive(1, 0.0, 20.0, 3900.0);
    h.end();
    let stored = laps(&h);
    assert_eq!(stored[0].kind, "flying");
    assert_eq!((stored[1].lap, stored[1].kind), (2, "in"));
    assert!(stored[1].record.lap_time_s.is_none());
}

#[test]
fn test_bug013_pausing_on_track_does_not_mark_an_in_lap() {
    let mut h = Harness::new();
    h.feed(1, 1.0, 0.0);
    h.drive(1, 0.0, 40.0, 0.0);
    h.sim.d.game_state = gs::INGAME_PAUSED;
    h.feed(1, 1800.0, 40.0);
    assert!(h.tracker.lap.as_ref().is_some_and(|l| !l.in_lap));
}

#[test]
fn test_bug013_quit_from_the_menu_on_track_stays_flying() {
    // The old behaviour where it is right: quitting from the menu while on track.
    let mut h = Harness::new();
    h.feed(1, 1.0, 0.0);
    h.drive(1, 0.0, 40.0, 0.0);
    h.sim.d.game_state = gs::INGAME_INMENU_TIME_TICKING;
    h.feed(1, 1800.0, 40.0);
    h.sim.d.game_state = gs::FRONT_END;
    h.feed(1, 1800.0, 40.0);
    assert_eq!(
        laps(&h).iter().map(|l| l.kind).collect::<Vec<_>>(),
        ["flying"]
    );
}

// ---- the session opens in the garage (a menu in AMS2), before the car leaves -----

/// A frame of the garage screen: AMS2 shows it as a menu with time ticking.
fn garage(h: &mut Harness, state: u32) {
    h.sim.d.game_state = state;
    h.sim.d.pit_mode = pm::IN_GARAGE;
    h.feed_kmh(1, 92.0, -1.0, 0.0);
}

#[test]
fn test_bug015_session_opens_in_the_garage_and_laps_are_unchanged() {
    let mut h = Harness::new();
    for _ in 0..3 {
        // the driver looks at the setup in the garage (a capture is taken)
        garage(&mut h, gs::INGAME_INMENU_TIME_TICKING);
        h.tick(1.0);
    }
    assert!(h.tracker.session.is_some() && h.tracker.lap.is_none());
    assert_eq!(h.sink.kinds()[..3], ["catalog", "session_start", "setup"]);
    assert!(
        h.sink
            .events()
            .iter()
            .any(|e| matches!(e, Event::SessionStarted { track, .. } if track == "Road_Atlanta"))
    );
    // Then out of the garage and on track: the same session, out lap and first crossing as
    // before (the lap counter does not move at the first crossing).
    h.sim.d.game_state = gs::INGAME_PLAYING;
    leave_garage(&mut h);
    h.drive(1, 0.0, 90.0, 1.0);
    h.sim.d.last_lap_time = f(90.0);
    h.feed(2, 1.0, 0.02);
    h.end();
    assert_eq!(h.sink.stored_sessions(), 1);
    let started = h
        .sink
        .events()
        .iter()
        .filter(|e| matches!(e, Event::SessionStarted { .. }))
        .count();
    assert_eq!(started, 1);
    let got = summary(&h);
    assert_eq!(got.len(), 2);
    assert_eq!((got[0].0, got[0].1, got[0].2), (1, "out", None));
    assert_eq!((got[1].0, got[1].1), (2, "flying"));
    assert!(near(got[1].2, 90.0));
    // The garage snapshot, then the one taken when the car left.
    let reasons: Vec<&str> = h.sink.setups().iter().map(|(r, _)| *r).collect();
    assert_eq!(reasons, ["session_start", "pit_exit"]);
}

#[test]
fn test_bug015_browsing_the_garage_without_driving_stores_nothing() {
    let mut h = Harness::new();
    garage(&mut h, gs::INGAME_PAUSED);
    h.tick(5.0);
    garage(&mut h, gs::FRONT_END); // back to the menus
    assert_eq!(h.sink.stored_sessions(), 0);
    assert_eq!(session_ended(&h), [(1, 0, true)]);
}

#[test]
fn test_bug015_no_session_outside_a_loaded_garage() {
    type Case = (&'static str, Box<dyn Fn(&mut SharedMemory)>);
    let cases: Vec<Case> = vec![
        (
            "paused on track: not the garage",
            Box::new(|d| d.pit_mode = pm::NONE),
        ),
        (
            "still loading: no layout yet",
            Box::new(|d| d.track_variation = [0; 64]),
        ),
        (
            "invalid session",
            Box::new(|d| d.session_state = ss::INVALID),
        ),
        ("no player car yet", Box::new(|d| d.num_participants = 0)),
        ("front end", Box::new(|d| d.game_state = gs::FRONT_END)),
    ];
    for (name, change) in cases {
        let mut h = Harness::new();
        h.sim.d.game_state = gs::INGAME_INMENU_TIME_TICKING;
        h.sim.d.pit_mode = pm::IN_GARAGE;
        change(&mut h.sim.d);
        h.feed_kmh(1, 92.0, -1.0, 0.0);
        assert!(h.tracker.session.is_none(), "{name}");
        assert!(h.sink.kinds().is_empty(), "{name}");
    }
}

#[test]
fn test_bug015_session_change_in_the_garage_opens_the_new_one() {
    // The event moves on while the car is in the garage (practice -> qualifying).
    let mut h = Harness::new();
    h.feed(1, 1.0, 0.0);
    h.drive(1, 0.0, 90.0, 0.0);
    h.sim.d.last_lap_time = f(90.0);
    h.feed(2, 1.0, 0.02);
    garage(&mut h, gs::INGAME_INMENU_TIME_TICKING);
    let first = h.tracker.session_id().unwrap();
    h.sim.d.session_state = ss::QUALIFY;
    garage(&mut h, gs::INGAME_INMENU_TIME_TICKING);
    assert_ne!(h.tracker.session_id().unwrap(), first);
    assert_eq!(h.tracker.session.as_ref().unwrap().session_type, "QUALIFY");
    assert!(h.tracker.lap.is_none());
    assert_eq!(h.sink.stored_sessions(), 1); // qualy: no lap yet
}

// ------------------------------------------------- live view of the open lap

const GRASS: u32 = 7;
const RUMBLE: u32 = 10;
const ILLEGAL_STRIP: u32 = 47;
const ROAD: u32 = 0;

/// Out lap from mid track, the line, then the flying lap up to `t_end` on the clock.
fn flying_lap_until(h: &mut Harness, t_end: f64) {
    h.sim.d.pit_mode = pm::NONE;
    h.drive(1, 40.0, 90.0, 2000.0);
    h.drive_step(2, 0.0, t_end, 0.0, 0.1);
}

fn run(h: &mut Harness, t0: f64, t1: f64, terrain: [u32; 4]) {
    h.sim.d.terrain = terrain;
    let mut t = t0;
    while t < t1 {
        h.feed(2, t / 90.0 * 4088.0, t);
        h.tick(0.1);
        t += 0.1;
    }
}

fn live(h: &Harness) -> LiveLap {
    h.tracker.live_lap().expect("an open lap")
}

#[test]
fn test_live_lap_out_lap_is_not_an_invalid_lap() {
    // Session 28: on an out lap the game sets the invalid flag with no lap clock; the live view
    // says out lap and gives no invalid cause.
    let mut h = Harness::new();
    assert!(h.tracker.live_lap().is_none());
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_PITS;
    h.sim.d.lap_invalidated = 1;
    h.feed(1, 2400.0, -1.0);
    h.tick(60.0);
    h.sim.d.pit_mode = pm::NONE;
    h.feed(1, 2500.0, -1.0);
    let l = live(&h);
    assert_eq!((l.lap_type, l.invalid_cause), ("out", None));
    // The line: the flying lap starts, the lingering flag inside the grace time is ignored.
    h.feed(2, 1.0, 0.1);
    h.sim.d.lap_invalidated = 0;
    h.feed(2, 20.0, 0.6);
    let l = live(&h);
    assert_eq!((l.lap_type, l.invalid_cause), ("flying", None));
}

#[test]
fn test_live_lap_off_track_cause() {
    let mut h = Harness::new();
    flying_lap_until(&mut h, 30.0);
    run(&mut h, 30.0, 30.5, [GRASS, GRASS, ROAD, ROAD]);
    run(&mut h, 30.5, 31.0, [ROAD; 4]);
    h.sim.d.lap_invalidated = 1;
    run(&mut h, 31.0, 31.2, [ROAD; 4]);
    let l = live(&h);
    assert_eq!((l.lap_type, l.invalid_cause), ("flying", Some("off_track")));
    assert!(l.invalid_cause_certain);
    assert!(
        l.invalid_at_m
            .is_some_and(|m| (m - 31.0 / 90.0 * 4088.0).abs() < 1.0)
    );
    // The cause is decided once, when the flag comes.
    run(&mut h, 31.2, 40.0, [RUMBLE; 4]);
    assert_eq!(live(&h).invalid_cause, Some("off_track"));
    // The stored lap is unchanged: invalid, flying.
    h.feed(3, 1.0, 0.02);
    h.tick(5.0);
    h.feed(3, 50.0, 1.0);
    assert!(!laps(&h).last().unwrap().record.valid);
}

#[test]
fn test_live_lap_invalid_causes() {
    let cases: [([u32; 4], usize, (&str, bool)); 7] = [
        ([RUMBLE; 4], 10, ("track_limits", false)), // all four on kerbs: only probable
        ([RUMBLE, RUMBLE, RUMBLE, GRASS], 10, ("track_limits", false)),
        ([RUMBLE; 4], 3, ("unknown", true)), // too short a touch
        (
            [ILLEGAL_STRIP, ILLEGAL_STRIP, ROAD, ROAD],
            2,
            ("track_limits", true),
        ),
        ([ILLEGAL_STRIP, GRASS, ROAD, ROAD], 2, ("off_track", true)),
        ([GRASS, ROAD, ROAD, ROAD], 10, ("unknown", true)), // one wheel off
        ([ROAD; 4], 10, ("unknown", true)),
    ];
    for (terrain, samples, cause) in cases {
        let mut h = Harness::new();
        flying_lap_until(&mut h, 30.0);
        run(&mut h, 30.0, 30.0 + samples as f64 * 0.1 - 0.05, terrain);
        h.sim.d.lap_invalidated = 1;
        run(&mut h, 31.0, 31.1, [ROAD; 4]);
        let l = live(&h);
        assert_eq!(
            (l.invalid_cause, l.invalid_cause_certain),
            (Some(cause.0), cause.1),
            "{terrain:?} x{samples}"
        );
    }
}

#[test]
fn test_live_lap_cause_window_and_pit_lane() {
    let mut h = Harness::new();
    flying_lap_until(&mut h, 10.0);
    run(&mut h, 10.0, 10.5, [GRASS; 4]); // 20 s before the flag
    run(&mut h, 10.5, 30.0, [ROAD; 4]);
    h.sim.d.lap_invalidated = 1;
    run(&mut h, 30.0, 30.2, [ROAD; 4]);
    assert_eq!(live(&h).invalid_cause, Some("unknown"));
}

#[test]
fn test_live_lap_in_lap_and_formation() {
    let mut h = Harness::new();
    flying_lap_until(&mut h, 30.0);
    h.sim.d.pit_mode = pm::DRIVING_INTO_PITS;
    run(&mut h, 30.0, 30.3, [ROAD; 4]);
    assert_eq!(live(&h).lap_type, "in");

    let mut h = Harness::new();
    h.sim.d.session_state = ss::RACE;
    h.feed(1, 0.0, -1.0); // on the grid, no lap clock
    let l = live(&h);
    assert_eq!((l.lap_type, l.invalid_cause), ("formation", None));
}

// ---- a lap that never moved is not a lap; in the car in the garage -------------------

/// Sitting in the car in the garage: the game is playing, "driving out of the garage" at 0 km/h,
/// lap clock -1 (sessions 30 and 31, 2026-10-18).
fn parked(h: &mut Harness, kmh: f64) {
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_GARAGE;
    h.feed_kmh(1, 62.3, -1.0, kmh);
}

#[test]
fn test_bug020_sitting_in_the_car_then_the_recorder_stops_discards_the_session() {
    // T21 (2026-10-18): garage screen, then in the car for 37 s without moving, then the
    // Recorder switched off: no lap, and the session is discarded.
    let mut h = Harness::new();
    garage(&mut h, gs::INGAME_INMENU_TIME_TICKING);
    h.sim.d.game_state = gs::INGAME_PLAYING;
    for i in 0..40 {
        parked(&mut h, if i % 7 == 0 { 0.9 } else { 0.0 }); // noise
        h.tick(1.0);
    }
    assert_eq!(h.tracker.lap.as_ref().unwrap().rows.len(), 40);
    h.end(); // the recorder stops
    assert!(laps(&h).is_empty() && h.sink.stored_sessions() == 0);
    assert_eq!(session_ended(&h), [(1, 0, true)]);
    assert!(lap_events(&h).is_empty());
}

#[test]
fn test_bug020_sitting_then_driving_out_keeps_the_out_lap() {
    let mut h = Harness::new();
    garage(&mut h, gs::INGAME_INMENU_TIME_TICKING);
    h.sim.d.game_state = gs::INGAME_PLAYING;
    for _ in 0..20 {
        parked(&mut h, 0.0);
        h.tick(1.0);
    }
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_GARAGE;
    h.drive_kmh(1, 0.0, 5.0, 62.3, 7.0);
    h.sim.d.pit_mode = pm::NONE;
    h.drive(1, 5.0, 40.0, 400.0);
    h.end();
    let stored = laps(&h);
    assert_eq!(stored.len(), 1);
    assert_eq!(
        (stored[0].lap, stored[0].kind, stored[0].record.lap_time_s),
        (1, "out", None)
    );
    assert!(near(stored[0].record.max_speed_kmh, 100.0));
    assert_eq!(h.sink.stored_sessions(), 1);
}

#[test]
fn test_bug020_a_moving_partial_lap_is_still_kept() {
    let mut h = Harness::new();
    h.drive_kmh(1, 0.0, 30.0, 0.0, 4.0); // slow, but moving
    h.end();
    let stored = laps(&h);
    assert_eq!(stored.len(), 1);
    assert!(near(stored[0].record.max_speed_kmh, 4.0));
}

#[test]
fn test_in_garage_screen_and_parked_in_the_car() {
    // The tracker tells the garage (screen, or parked in the car before it moved) from driving
    // out of it; the lap rules are unchanged.
    let mut h = Harness::new();
    garage(&mut h, gs::INGAME_INMENU_TIME_TICKING);
    assert!(h.tracker.in_garage());
    h.sim.d.game_state = gs::INGAME_PLAYING;
    parked(&mut h, 0.0);
    assert!(h.tracker.in_garage() && live(&h).lap_type == "out");
    parked(&mut h, 7.0); // rolling out of the garage
    assert!(!h.tracker.in_garage());
    parked(&mut h, 0.0); // stopped again before the pit lane
    assert!(!h.tracker.in_garage());
    h.sim.d.pit_mode = pm::DRIVING_OUT_OF_PITS;
    h.feed_kmh(1, 300.0, -1.0, 0.0); // waiting in the lane
    assert!(!h.tracker.in_garage());
    garage(&mut h, gs::INGAME_INMENU_TIME_TICKING); // back to the garage screen, then in the car again
    h.sim.d.game_state = gs::INGAME_PLAYING;
    parked(&mut h, 0.0);
    assert!(h.tracker.in_garage());
}

#[test]
fn test_session_start_message_names_the_ids() {
    // Port of the catalog assertions of test_full_flow on the typed message.
    let mut h = Harness::new();
    h.feed(1, 1.0, 0.0);
    let Call::Start(info) = &h.sink.calls[0] else {
        panic!()
    };
    let (cat, start) = session_start_message(info, Names::embedded());
    let cat = cat.unwrap();
    assert_eq!(cat.layouts[0].layout_id, "Full");
    assert_eq!(cat.layouts[0].length_m, Some(4088.0));
    assert_eq!(start.session_type, "practice");
    assert_eq!(start.car_id.as_deref(), Some("MINI Cooper S 1965"));
}

mod robustness {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Whatever the game writes, the tracker neither panics nor loses the invariant that
        /// a stored lap has samples and a number from 1.
        #[test]
        fn random_frames_never_panic(frames in prop::collection::vec(
            (0u32..9, 0u32..8, 0u32..7, 0u32..4, 0u32..200, -2.0f32..5000.0, -2.0f32..300.0, -200.0f32..200.0), 1..300)) {
            let mut h = Harness::new();
            for (gs, pit, lap, sess, _n, dist, t, kmh) in frames {
                h.sim.d.game_state = gs;
                h.sim.d.pit_mode = pit;
                h.sim.d.session_state = sess;
                h.feed_kmh(lap, f64::from(dist), f64::from(t), f64::from(kmh));
                h.tick(0.5);
            }
            h.end();
            prop_assert!(h.sink.laps().iter().all(|l| !l.rows.is_empty() && l.lap >= 1));
        }
    }
}

/// Standing start: the game holds `mLapInvalidated` on the grid, with the lap clock
/// running, the distance at 0 and the sector at the last one, and clears it at the line.
fn standing_start(h: &mut Harness) {
    h.sim.d.session_state = ss::RACE;
    for _ in 0..10 {
        h.feed_kmh(1, 0.0, -1.0, 0.0);
        h.tick(1.0);
    }
    for i in 0..5 {
        // lights out, the car rolls behind the line: clock running, distance stuck at 0
        h.sim
            .frame_kmh(1, 0.0, f64::from(i) + 0.02, 20.0 * f64::from(i));
        h.sim.d.participant_info[0].current_sector = 2;
        h.sim.d.lap_invalidated = 1;
        h.sim.d.laps_invalidated[0] = 1;
        h.feed_current();
        h.tick(1.0);
    }
    // The line: the game clears the flag and the distance starts counting.
    h.sim.d.lap_invalidated = 0;
    h.sim.d.laps_invalidated[0] = 0;
    h.feed(1, 1.0, 9.6);
    h.tick(0.02);
}

#[test]
fn test_standing_start_grid_flag_does_not_invalidate_lap_1() {
    let mut h = Harness::new();
    standing_start(&mut h);
    h.drive(1, 10.0, 125.0, 2.0);
    h.sim.d.last_lap_time = f(125.38);
    h.feed(2, 1.0, 0.02);
    h.drive(2, 1.0, 20.0, 40.0);
    h.end();
    let stored = laps(&h);
    let lap1 = stored.iter().find(|l| l.kind == "flying").unwrap();
    assert!(near(lap1.record.lap_time_s, 125.38));
    assert!(lap1.record.valid);
}

#[test]
fn test_standing_start_lap_1_still_invalid_after_the_line() {
    let mut h = Harness::new();
    standing_start(&mut h);
    h.drive(1, 10.0, 60.0, 2.0);
    h.sim.d.lap_invalidated = 1; // a cut after the line
    h.drive(1, 60.0, 125.0, 1800.0);
    h.sim.d.last_lap_time = f(125.38);
    h.feed(2, 1.0, 0.02);
    h.drive(2, 1.0, 20.0, 40.0);
    h.end();
    let stored = laps(&h);
    let lap1 = stored.iter().find(|l| l.kind == "flying").unwrap();
    assert!(!lap1.record.valid);
}
