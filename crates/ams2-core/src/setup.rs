//! AMS2 setup snapshot to a `setup` message (`source: telemetry`).
//!
//! The recorder takes a snapshot of what AMS2's shared memory shows of the setup at the session
//! start and at every pit exit ([`SetupSnapshot`]; the keys are those of the Python dict).
//! Only what the driver sets becomes the setup:
//!
//! * brake bias to `brake_bias_front_pct`;
//! * compounds to `compound_front` (FL) / `compound_rear` (RL); when the left and right tyres
//!   of an axle differ, all four go in `extra` too (`compound_fl` ...);
//! * pressures to `pressure_*_kpa` **only when `pressures_cold` is true** (a hot or pre-heated
//!   reading is not the pressure set in the garage);
//! * `fuel_l`;
//! * wings, ABS and TC only when the car exposes them.
//!
//! Carcass temperatures, ambient and the fuel capacity are readings, not setup: left out.

use crate::messages::{ExtraEntry, ExtraValue, Setup};
use serde::Serialize;
use std::collections::BTreeMap;

/// Tyres count as cold when every carcass is within this of the ambient temperature.
pub const COLD_TYRE_MARGIN_C: f64 = 10.0;

const WHEELS: [&str; 4] = ["fl", "fr", "rl", "rr"];

/// What the shared memory shows of the setup at one moment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SetupSnapshot {
    /// Front brake bias, % (`None` when unset).
    pub brake_bias_front_pct: Option<f64>,
    /// ABS setting (-1 = the car has none).
    pub abs_setting: i32,
    /// TC setting (-1 = the car has none).
    pub tc_setting: i32,
    /// Wings `[front, rear]` (all zero = not exposed).
    pub wings: [f64; 2],
    /// Compounds FL, FR, RL, RR.
    pub tyre_compound: [String; 4],
    /// Pressures FL, FR, RL, RR (kPa).
    pub tyre_pressure_kpa: [f64; 4],
    /// Carcass temperatures (Celsius).
    pub tyre_carcass_c: [f64; 4],
    /// Ambient temperature.
    pub ambient_c: f64,
    /// Tyres at ambient temperature: the pressures are the cold setting.
    pub pressures_cold: bool,
    /// Fuel on board.
    pub fuel_l: f64,
    /// Tank capacity.
    pub fuel_capacity_l: f64,
}

fn compound(v: &str) -> Option<String> {
    let t = v.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// The `setup` message of one snapshot (`reason`: `session_start` / `pit_exit`).
pub fn setup_message(snapshot: &SetupSnapshot, taken_at: &str, reason: &str) -> Setup {
    let mut out = Setup {
        taken_at: taken_at.to_owned(),
        reason: reason.to_owned(),
        source: "telemetry".to_owned(),
        ..Setup::default()
    };
    let mut extra = BTreeMap::new();

    if let Some(b) = snapshot
        .brake_bias_front_pct
        .filter(|b| (0.0..=100.0).contains(b))
    {
        out.brake_bias_front_pct = Some(b);
    }

    let [fl, fr, rl, rr] = [0, 1, 2, 3].map(|i| compound(&snapshot.tyre_compound[i]));
    out.compound_front = fl.clone();
    out.compound_rear = rl.clone();
    if fl != fr || rl != rr {
        for (w, c) in WHEELS.iter().zip([&fl, &fr, &rl, &rr]) {
            if let Some(c) = c {
                extra.insert(
                    format!("compound_{w}"),
                    ExtraEntry {
                        label: format!("Tyre compound {}", w.to_uppercase()),
                        value: ExtraValue::Text(c.clone()),
                        group: Some("Tyres".into()),
                        unit: None,
                    },
                );
            }
        }
    }

    if snapshot.pressures_cold && snapshot.tyre_pressure_kpa.iter().all(|v| *v > 0.0) {
        let p = snapshot.tyre_pressure_kpa;
        out.pressure_fl_kpa = Some(p[0]);
        out.pressure_fr_kpa = Some(p[1]);
        out.pressure_rl_kpa = Some(p[2]);
        out.pressure_rr_kpa = Some(p[3]);
    }

    if snapshot.fuel_l >= 0.0 {
        out.fuel_l = Some(snapshot.fuel_l);
    }

    let [front, rear] = snapshot.wings;
    if front != 0.0 || rear != 0.0 {
        out.wing_front = Some(front);
        out.wing_rear = Some(rear);
    }

    if snapshot.abs_setting >= 0 {
        out.abs = Some(f64::from(snapshot.abs_setting));
    }
    if snapshot.tc_setting >= 0 {
        out.tc = Some(f64::from(snapshot.tc_setting));
    }
    out.extra = extra;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> SetupSnapshot {
        SetupSnapshot {
            brake_bias_front_pct: Some(80.0),
            abs_setting: 0,
            tc_setting: -1,
            wings: [0.0, 0.0],
            tyre_compound: [
                "Slick".into(),
                "Slick".into(),
                "Slick".into(),
                "Slick".into(),
            ],
            tyre_pressure_kpa: [174.0, 174.0, 192.0, 192.0],
            tyre_carcass_c: [20.0; 4],
            ambient_c: 20.0,
            pressures_cold: true,
            fuel_l: 25.5,
            fuel_capacity_l: 51.0,
        }
    }

    #[test]
    fn cold_snapshot_gives_pressures_and_leaves_out_what_the_car_lacks() {
        let m = setup_message(&snap(), "T", "session_start");
        assert_eq!(
            (m.pressure_fl_kpa, m.pressure_rr_kpa),
            (Some(174.0), Some(192.0))
        );
        assert_eq!((m.abs, m.tc, m.wing_front), (Some(0.0), None, None));
        assert_eq!((m.brake_bias_front_pct, m.fuel_l), (Some(80.0), Some(25.5)));
        assert!(m.extra.is_empty());
    }

    #[test]
    fn hot_tyres_give_no_pressures_and_mixed_compounds_go_to_extra() {
        let mut s = snap();
        s.pressures_cold = false;
        s.tyre_compound[1] = "Wet".into();
        let m = setup_message(&s, "T", "pit_exit");
        assert_eq!(m.pressure_fl_kpa, None);
        assert_eq!(m.extra.len(), 4);
        assert_eq!(m.extra["compound_fr"].label, "Tyre compound FR");
        assert_eq!(m.compound_front.as_deref(), Some("Slick"));
    }
}
