//! AMS2's own enum values (`SharedMemory.h`: game state, session state, race state, pit mode,
//! car flags), as constants for the raw `u32` fields.

/// Game states (`mGameState`).
pub mod game_state {
    /// The game exited.
    pub const EXITED: u32 = 0;
    /// Front end (main menus).
    pub const FRONT_END: u32 = 1;
    /// Playing.
    pub const INGAME_PLAYING: u32 = 2;
    /// Paused.
    pub const INGAME_PAUSED: u32 = 3;
    /// In a menu, time ticking (the garage screen).
    pub const INGAME_INMENU_TIME_TICKING: u32 = 4;
    /// Restarting.
    pub const INGAME_RESTARTING: u32 = 5;
    /// Replay.
    pub const INGAME_REPLAY: u32 = 6;
    /// Front end replay.
    pub const FRONT_END_REPLAY: u32 = 7;
}

/// Session states (`mSessionState`).
pub mod session_state {
    /// Invalid / unknown.
    pub const INVALID: u32 = 0;
    /// Practice.
    pub const PRACTICE: u32 = 1;
    /// Test.
    pub const TEST: u32 = 2;
    /// Qualifying.
    pub const QUALIFY: u32 = 3;
    /// Formation lap.
    pub const FORMATION_LAP: u32 = 4;
    /// Race.
    pub const RACE: u32 = 5;
    /// Time attack.
    pub const TIME_ATTACK: u32 = 6;
    /// The names, by value.
    pub const NAMES: [&str; 7] = [
        "INVALID",
        "PRACTICE",
        "TEST",
        "QUALIFY",
        "FORMATION_LAP",
        "RACE",
        "TIME_ATTACK",
    ];
}

/// Pit modes (`mPitMode`).
pub mod pit_mode {
    /// On track.
    pub const NONE: u32 = 0;
    /// Driving into the pits.
    pub const DRIVING_INTO_PITS: u32 = 1;
    /// In the pit box.
    pub const IN_PIT: u32 = 2;
    /// Driving out of the pits.
    pub const DRIVING_OUT_OF_PITS: u32 = 3;
    /// In the garage.
    pub const IN_GARAGE: u32 = 4;
    /// Driving out of the garage.
    pub const DRIVING_OUT_OF_GARAGE: u32 = 5;
}

/// Race states (`mRaceStates`).
pub mod race_state {
    /// Racing.
    pub const RACING: u32 = 2;
}

/// Car flags (`mCarFlags`).
pub mod car_flags {
    /// ABS intervening.
    pub const ABS: u32 = 1 << 4;
    /// Traction control intervening.
    pub const TCS: u32 = 1 << 6;
}
