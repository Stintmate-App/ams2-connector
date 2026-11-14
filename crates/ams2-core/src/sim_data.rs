//! The simulator data this connector writes (`pitwall.write.sim_data`) and how it describes it
//! (`pitwall.write.sim_data_kinds`), so the race engineer reads a record without guessing.
//!
//! One kind so far: [`DAMAGE`].

use std::collections::BTreeMap;
use stintmate_connector_wire::messages::{SimDataField, SimDataKind};

/// The kind of the car's damage record.
pub const DAMAGE: &str = "damage";

fn field(meaning: &str, unit: Option<&str>) -> SimDataField {
    SimDataField {
        meaning: meaning.to_owned(),
        unit: unit.map(str::to_owned),
    }
}

/// The description of every kind this connector writes.
pub fn kinds() -> Vec<SimDataKind> {
    let fraction = "0 to 1 (the game's range); 0 is healthy (the game does not state it; it matches \
                    every known reader and our recordings)";
    vec![SimDataKind {
        kind: DAMAGE.to_owned(),
        description: "The car's damage as AMS2 reports it: aero and engine, and brakes and \
                      suspension per wheel, each from 0 to 1 (the game's range). The game does \
                      not document which end is healthy; every known reader and our recordings \
                      show 0 healthy, growing with damage. A record is written about 1 s after \
                      the values settle on new ones (a hit, a repair, the game's damage pit \
                      request set or cleared); the latest record is the car's state. Not live."
            .to_owned(),
        topic: Some("car".to_owned()),
        fields: BTreeMap::from([
            (
                "aero".to_owned(),
                field(&format!("Aero damage, {fraction}"), Some("fraction")),
            ),
            (
                "engine".to_owned(),
                field(&format!("Engine damage, {fraction}"), Some("fraction")),
            ),
            (
                "wheels".to_owned(),
                field("Per wheel: fl, fr, rl, rr (front left to rear right)", None),
            ),
            (
                "wheels.*.brake".to_owned(),
                field(
                    &format!("Brake damage of the wheel, {fraction}"),
                    Some("fraction"),
                ),
            ),
            (
                "wheels.*.suspension".to_owned(),
                field(
                    &format!("Suspension damage of the wheel, {fraction}"),
                    Some("fraction"),
                ),
            ),
            (
                "wheels.*.attached".to_owned(),
                field("The game's tyre flag: the wheel is on the car", None),
            ),
            (
                "wheels.*.inflated".to_owned(),
                field(
                    "The game's tyre flag: the tyre holds air; not yet seen false in a recording",
                    None,
                ),
            ),
            (
                "pit_request".to_owned(),
                field(
                    "The game's pit schedule by its SharedMemory.h name; \
                     PIT_SCHEDULE_DAMAGE_REQUESTED: the game asks the car to pit for repairs. \
                     The request may stay after the car was repaired another way: compare the values",
                    None,
                ),
            ),
        ]),
    }]
}
