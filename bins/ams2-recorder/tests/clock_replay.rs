//! The session clock across the scripted session (menus, garage as a menu, out lap,
//! flying laps, a pit stop, a return to the garage mid lap). The script is the replay fixture of
//! the Python golden, not a recording of the game: it shows the rules agree with a session that
//! is stopped and resumed at any two frames, and refuse one whose clock restarts.

use ams2_core::testing::scripted_session;
use ams2_shm::SessionClock;

#[test]
fn any_two_frames_of_the_one_session_continue() {
    let clocks: Vec<SessionClock> = scripted_session()
        .iter()
        .filter(|f| f.frame.car_name[0] != 0)
        .map(|f| SessionClock::from_frame(&f.frame))
        .collect();
    assert!(clocks.len() > 100, "the script has a session");
    let timed = clocks.iter().filter(|c| c.time_remaining_s >= 0.0).count();
    assert!(timed > 100, "its clock counts down");
    // stop at one frame, resume at any later one, in steps to keep the test quick
    for (i, stop) in clocks.iter().enumerate().step_by(37) {
        for now in clocks[i..].iter().step_by(53) {
            assert!(
                stop.continues_as(now),
                "{stop:?} should continue as {now:?}"
            );
        }
    }
}

#[test]
fn the_same_script_started_again_is_a_new_session() {
    let clocks: Vec<SessionClock> = scripted_session()
        .iter()
        .filter(|f| f.frame.car_name[0] != 0)
        .map(|f| SessionClock::from_frame(&f.frame))
        .collect();
    let (first, last) = (clocks[0], clocks[clocks.len() - 1]);
    assert!(
        last.laps_completed > first.laps_completed
            || last.time_remaining_s < first.time_remaining_s,
        "the script makes progress"
    );
    // stopped at the end, then the same session type opened again from the beginning
    assert!(!last.continues_as(&first));
}
