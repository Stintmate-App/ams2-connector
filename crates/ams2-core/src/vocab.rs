//! AMS2 values to Pitwall's vocabularies: session types, surfaces, pit modes, race states.
//! Also the neutral codes the connector writes (Pitwall's data model).

/// Surface code: road.
pub const SURFACE_ROAD: u8 = 0;
/// Surface code: kerb.
pub const SURFACE_KERB: u8 = 1;
/// Surface code: track-limit strip.
pub const SURFACE_STRIP: u8 = 6;
/// Surface code: wall.
pub const SURFACE_WALL: u8 = 7;

/// Pitwall session type of AMS2's `mSessionState` value.
pub fn session_type(state: u32) -> &'static str {
    match state {
        1 | 2 | 6 => "practice",
        3 => "qualifying",
        4 | 5 => "race",
        _ => "other",
    }
}

/// Pitwall session type of an AMS2 session state *name* (`"QUALIFY"`, `"UNKNOWN_9"`).
pub fn session_type_of_name(name: &str) -> &'static str {
    match name {
        "PRACTICE" | "TEST" | "TIME_ATTACK" => "practice",
        "QUALIFY" => "qualifying",
        "FORMATION_LAP" | "RACE" => "race",
        _ => "other",
    }
}

/// AMS2 `mTerrain` material to the surface code. Kerbs, off-track and walls are the sets the
/// analysis used on AMS2 codes; every material not listed is racing surface (0).
pub fn surface(terrain: f64) -> u8 {
    match terrain as i64 {
        10 | 25 | 40 | 41 => SURFACE_KERB,
        6 | 7 | 26..=28 => 2,
        8 | 9 => 3,
        15 | 16 | 42 | 43 => 4,
        46 => 5,
        47 | 49 => SURFACE_STRIP,
        12..=14 => SURFACE_WALL,
        17..=20 | 22 | 24 => 8,
        _ => SURFACE_ROAD,
    }
}

/// AMS2 `mPitMode` to the pit_mode code (driving out of the garage is driving out; undefined
/// values count as in the pits).
pub fn pit_mode(mode: f64) -> u8 {
    if mode.fract() != 0.0 {
        return 2;
    }
    match mode as i64 {
        0 => 0,
        1 => 1,
        2 => 2,
        3 | 5 => 3,
        4 => 4,
        _ => 2,
    }
}

/// Race state label (as the rival reports print it).
pub fn race_state(value: u32) -> String {
    const NAMES: [&str; 7] = [
        "INVALID",
        "NOT_STARTED",
        "RACING",
        "FINISHED",
        "DISQUALIFIED",
        "RETIRED",
        "DNF",
    ];
    NAMES
        .get(value as usize)
        .map_or_else(|| value.to_string(), |s| (*s).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces() {
        let got: Vec<u8> = [46.0, 40.0, 0.0, 7.0, 47.0, 13.0, 18.0, 99.0]
            .iter()
            .map(|&t| surface(t))
            .collect();
        assert_eq!(got, [5, 1, 0, 2, 6, 7, 8, 0]);
    }

    #[test]
    fn pit_modes_and_types() {
        assert_eq!(pit_mode(5.0), 3);
        assert_eq!(pit_mode(9.0), 2);
        assert_eq!(session_type(4), "race");
        assert_eq!(session_type(0), "other");
        assert_eq!(session_type(9), "other");
        assert_eq!(session_type_of_name("UNKNOWN_9"), "other");
        assert_eq!(race_state(2), "RACING");
        assert_eq!(race_state(12), "12");
    }
}
