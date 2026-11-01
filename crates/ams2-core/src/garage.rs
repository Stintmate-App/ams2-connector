//! In the car, in the garage: what AMS2's pit mode alone does not say (FINDINGS "In the car in the garage").
//!
//! The garage screen is a menu with the pit mode "in the garage". Once the driver presses
//! Drive he sits in the car, still in the garage, and the game is "playing" with the pit mode
//! "driving out of the garage" and the speed at 0 (up to ~1 km/h of noise), for as long as he
//! waits; then the same pit mode while the car rolls out. So "driving out of the garage" and
//! not moving yet means the car is still in the garage; once it moved it is driving out, also
//! if it stops again (the latch keeps it from flickering at walking pace).

use crate::codes::{game_state, pit_mode};

/// Faster than this the car is moving: the parked car reads up to ~1 km/h of noise.
pub const MOVING_KMH: f64 = 3.0;

/// The protocol's `phase`: `menus`, `garage`, `on_track` or `paused`. `in_garage`: a
/// [`GarageSeat`] says the car is in the garage, which with the pit mode "driving out of the
/// garage" means parked in the car there.
pub fn phase(game: u32, pit: u32, in_garage: bool) -> &'static str {
    let base = match game {
        game_state::INGAME_PLAYING => "on_track",
        game_state::INGAME_PAUSED | game_state::INGAME_INMENU_TIME_TICKING => "paused",
        _ => "menus",
    };
    let parked = in_garage && pit == pit_mode::DRIVING_OUT_OF_GARAGE;
    let screen = base == "paused" && pit == pit_mode::IN_GARAGE;
    if base != "menus" && (parked || screen) {
        "garage"
    } else {
        base
    }
}

/// Follows one car's pit mode and speed, frame by frame.
#[derive(Debug, Default, Clone)]
pub struct GarageSeat {
    /// In the car, in the garage, not moved yet.
    pub parked: bool,
    /// The garage screen, or parked in the car there.
    pub in_garage: bool,
    prev: Option<u32>,
}

impl GarageSeat {
    /// A seat with nothing known.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the next frame (`speed_ms` in m/s); returns whether the car is in the garage.
    pub fn update(&mut self, pit: u32, speed_ms: f64) -> bool {
        if pit == pit_mode::DRIVING_OUT_OF_GARAGE {
            let still = speed_ms * 3.6 < MOVING_KMH;
            self.parked =
                still && (self.parked || self.prev != Some(pit_mode::DRIVING_OUT_OF_GARAGE));
        } else {
            self.parked = false;
        }
        self.prev = Some(pit);
        self.in_garage = self.parked || pit == pit_mode::IN_GARAGE;
        self.in_garage
    }

    /// The game is gone (or a new mapping): nothing known.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::pit_mode::*;

    fn kmh(v: f64) -> f64 {
        v / 3.6
    }

    #[test]
    fn garage_screen_then_parked_then_driving_out() {
        let mut seat = GarageSeat::new();
        assert!(seat.update(IN_GARAGE, 0.0) && !seat.parked);
        assert!(seat.update(DRIVING_OUT_OF_GARAGE, kmh(0.0)) && seat.parked);
        assert!(seat.update(DRIVING_OUT_OF_GARAGE, kmh(1.0))); // the parked car's noise
        assert!(!seat.update(DRIVING_OUT_OF_GARAGE, kmh(MOVING_KMH))); // it moves
        assert!(!seat.update(DRIVING_OUT_OF_GARAGE, kmh(0.0))); // stopped again: still out
        assert!(!seat.update(DRIVING_OUT_OF_PITS, kmh(40.0)));
        assert!(!seat.update(NONE, kmh(150.0)));
        assert!(!seat.in_garage);
    }

    #[test]
    fn first_frame_already_in_the_car_counts_as_the_garage() {
        let mut seat = GarageSeat::new();
        assert!(seat.update(DRIVING_OUT_OF_GARAGE, kmh(0.0)) && seat.in_garage);
    }

    #[test]
    fn pit_box_and_pit_lane_are_not_the_garage() {
        let mut seat = GarageSeat::new();
        for mode in [DRIVING_INTO_PITS, IN_PIT, DRIVING_OUT_OF_PITS] {
            assert!(!seat.update(mode, kmh(0.0)));
        }
    }

    #[test]
    fn reset() {
        let mut seat = GarageSeat::new();
        seat.update(DRIVING_OUT_OF_GARAGE, 0.0);
        seat.reset();
        assert!(!seat.in_garage && !seat.parked);
        assert!(seat.update(DRIVING_OUT_OF_GARAGE, 0.0)); // a new first frame
    }

    #[test]
    fn phase_table() {
        use crate::codes::game_state::*;
        let (playing, paused, menu) = (INGAME_PLAYING, INGAME_PAUSED, INGAME_INMENU_TIME_TICKING);
        assert_eq!(phase(menu, IN_GARAGE, false), "garage");
        assert_eq!(phase(paused, IN_GARAGE, false), "garage");
        assert_eq!(phase(paused, NONE, false), "paused");
        assert_eq!(phase(menu, IN_PIT, false), "paused");
        assert_eq!(phase(playing, DRIVING_OUT_OF_GARAGE, true), "garage");
        assert_eq!(phase(paused, DRIVING_OUT_OF_GARAGE, true), "garage");
        assert_eq!(phase(playing, DRIVING_OUT_OF_GARAGE, false), "on_track");
        assert_eq!(phase(playing, IN_GARAGE, true), "on_track");
        assert_eq!(phase(playing, NONE, false), "on_track");
        for state in [FRONT_END, EXITED, INGAME_REPLAY] {
            assert_eq!(phase(state, IN_GARAGE, true), "menus");
        }
    }
}
