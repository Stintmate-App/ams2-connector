# AMS2 findings

Research and facts about Automobilista 2 (AMS2) that the connector relies on, with the
evidence behind them: shared memory, game quirks, game files, calibrations and validation
results. They were gathered from October 2026 on, by reading the game's own header, recording
real sessions and comparing the recording with the game's official times; later findings carry
their date. If you are bringing another simulator to Stintmate, this file shows what kind of
questions to ask of a simulator and how to answer them with evidence
([writing-a-connector.md](writing-a-connector.md)).

How the connector uses these facts: [ARCHITECTURE.md](ARCHITECTURE.md). Session numbers in the
older notes were ids in a development database; they are kept only to tell observations apart.

## Shared memory

### Setup and access

- Enable in AMS2: Options > System > Shared Memory = "Project CARS 2". The mapping is named
  `$pcars2$`.
- The official header is the source of truth: `SharedMemory.h` (and its `App.cpp`) in the
  game's `Support\SharedMemory\AMS2_SharedMemoryExampleApp` folder.
  `SHARED_MEMORY_VERSION = 14`.
- There is no `#pragma pack`: natural alignment (no packing), `bool` is 1 byte plus padding.
  Size **20 700 bytes** (computed from the header and pinned by a compile-time assertion;
  CrewChief measured 20 692 in an older version without the last two fields).
- Updated once per rendered frame.
- Consistent reads: the `mSequenceNumber` pattern (odd = being written; read again until the
  value is the same before and after the copy).
- Open it with `OpenFileMappingW(FILE_MAP_READ)`, never by creating the mapping by name (the
  latter creates an empty mapping when the game is not running).
- An open handle keeps the mapping alive after the game exits (frozen data), and CrewChief,
  Zero to Apex and the connector's own processes all hold one: the mapping's existence does
  not say the game runs. The connector checks the game's process instead (`AMS2AVX.exe` /
  `AMS2.exe`, [ARCHITECTURE.md](ARCHITECTURE.md#special-cases-and-why)).
- UDP (port 5606) exists but is not used: more complex, with network and CPU cost.

### Units and meanings

- `mSpeed` is m/s; `mFuelLevel` a fraction 0-1 (times `mFuelCapacity` = litres).
- `mAirPressure` is **kPa**, not PSI as the header says. Verified: a setup of 1.74 bar front /
  1.92 bar rear read about 175-180 front (warm) and 192 rear in telemetry.
- `mBrakeBias` is the **rear** fraction. Verified: setup 80.0 / 20.0 (front / rear) read 0.20.
  Front percent = `(1 - mBrakeBias) * 100`.
- `mRideHeight` is cm; suspension travel is in m.
- `mTyreTreadTemp`, `mTyreCarcassTemp`, `mTyreInternalAirTemp` (and the layer and rim temps)
  are in **Kelvin**; `mTyreTempLeft/Center/Right` and `mBrakeTempCelsius` are in degrees C.
- `mEventTimeRemaining` is in **seconds** (the header says ms; in a capture of 2026-10-08 it
  drops about 1 per second).
- `mSplitTimeAhead/Behind` are not on-track gaps: they follow the classification neighbours
  and are -1 for the leader.
- Wheel order is [FL, FR, RL, RR]. `mTyreRPS` is negative when moving forward.
- UNSET is -1 for times and `mBrakeBias`; strings are `char[64]`, NUL-terminated (UTF-8).
- `mWings` = 0 and `mAntiLockSetting` / `mTractionControlSetting` = -1 on a car without aero
  and without ABS / TC: expected, not missing data.
- Obsolete in the header: `mTyreSlipSpeed`, `mTyreGrip`, `mTyreLateralStiffness`. **The full
  setup and the weather forecast are not exposed.** Only brake bias, pressures, fuel, wings,
  compound and the ABS / TC settings are visible.

### Orientation and signs

- **Tyre temperature orientation** (inferred from `App.cpp`, confirmed live on 2026-10-04):
  "Left" / "Right" are the car's left and right sides. FL is hotter on `Right`, FR hotter on
  `Left`: both towards the car centreline (inner edge, negative camber), on straights and in
  corners of both directions. So on FL / RL `Left` = **outer**; on FR / RR `Left` = **inner**.
  Critical for camber analysis.
- `mUnfilteredSteering` > 0 = steering right (confirmed on a clockwise track, which has about
  2.5 times more time with steer > 0). `mLocalAcceleration[0]` has the opposite sign to the
  steering in corners, that is, positive towards the car's left; the connector stores lateral
  g with the steering's sign.
- World position: (x, z) with z up is the correct, non-mirrored plan view on the recorded
  tracks (checked against lateral g while drawing track maps).

### Update behaviour (2026-10-04)

- Works with another reader (Zero to Apex) running at the same time; no conflict.
- Update rate while driving: about 90 writes per second (the VR frame rate). One torn read in
  3 minutes, handled by the sequence check.
- In menus the game does not write per frame and `mSequenceNumber` sits at 0; it starts
  counting when a session loads.
- Tread, carcass, layer, rim and internal-air temps, ride height, suspension travel and brake
  temps are all populated and plausible.

### Participants (AI cars)

Feasibility capture of 2026-10-08 (2 Hz, 18 AI cars, a practice session). AMS2 fills, for AI
cars: last and fastest lap, current-lap sector times, position, laps completed, pit mode and
the invalid flag.

- There is a **"Safety Car" participant** (name "Safety Car", class "SafetyCar") to exclude.
- `mCurrentSector` is 0-based and goes 2 to 0 at **every** line crossing (pit lane and out-lap
  crossings too), but `mLapsCompleted` does not count an out lap or a practice pit-lane
  crossing (AI cars and player alike): those crossings only start a new lap.
- **Sector-time semantics:** the current sector's `mCurrentSector*Times` value is a running
  clock that becomes final when the sector index changes; the other sectors linger from the
  previous lap. Pit entry sets all three to -1 and the invalid flag. Unset lap times are -123.
- `mLastLapTimes[i]` changes in the same frame as the lap counter at 2 Hz; at 50 Hz it may lag
  like the player's `mLastLapTime`.
- Every AI best in the replayed capture equals the game's fastest lap.

## Game quirks

- **The lap counter is not incremented on the first crossing after the garage or pits.** Seen
  in every recorded session: the lap clock goes -1 to 0 and the distance wraps, but
  `mCurrentLap` stays at 1 until the next crossing. This merged the out lap and the first
  flying lap until lines were also detected by the distance wrap.
- **Player sector times:** the previous lap's sector values linger at the start of a lap; on an
  aborted lap the game writes the running time into the sector, so values equal to the lap
  clock of the last timed sample are not real sector times.
- **`mLastLapTime` lags the crossing**; the recorder waits up to 3 s for it. Without it, the lap
  clock at the last sample matches the official time closely (see "Calibrations").
- **The lap clock in the garage** resets to -1; partial-lap durations need an external clock.
- **Race start:** before the start the lap clock is -1 (grid, formation lap) and the lap
  distance can stay at 0.
- **Race loading with a stale lap counter:** a race can load still reporting the previous
  session's lap counter (qualifying's 4, then 1 for the formation lap, observed 2026-10-07).
  This used to create an empty extra session thirty seconds before the real race.
- **Pre-heated tyres at a race start:** AMS2 pre-heats tyres at a race start (carcass 45 C and
  1.83 bar instead of the cold 1.74), so race-start snapshots are not cold setup values.
  Likewise after a pit stop the tyres are still hot (2.07 bar, carcass 64-73 C on 2026-10-08).
- **Refuelling in the pit lane:** a pit-lane stop with refuelling changes the fuel (2.78 L at
  pit entry plus about 11.2 L = 13.94 L at pit exit). Fuel at lap end must be taken before
  stopping in the box or garage.
- **Event weather:** the air and track temperatures of a race can differ from those of its
  qualifying session on the same weekend (random weather per session).
- **Returning to the garage via the menu** keeps the session and the game lap counter: it is a
  new stint, not a new session.
- **The lap counter through the pits and the garage:** `mCurrentLap` counts the laps that count
  for the session, not line crossings. Observed 2026-10-18: the in lap was game lap 3, the out
  lap after the garage game lap 3, and the next flying lap **still** 3 (the out lap's crossing
  does not count), then 4. A connector that needs its own lap numbers has to number laps
  itself.

### In the car in the garage

The garage screen is a menu (paused, or "in menu, time ticking") with the pit mode **in the
garage** (4). Once the driver presses Drive he sits in the car, still in the garage, and the
game is **playing** with the pit mode **driving out of the garage** (5) and the speed at 0
(noise up to about 1 km/h), the lap clock at -1 and the gear in neutral, for as long as he
waits; the car rolling out keeps pit mode 5 (about 7 km/h within the first second, up to about
60 km/h), then **driving out of the pits** (3) in the lane, then none (0). There is no separate
"in the car in the garage" value in the header (`SharedMemory.h` v14: pit modes none, into the
pits, in the pit, out of the pits, in the garage, out of the garage), and no game state for it.

Evidence: every garage exit in the recordings of 2026-10-04 to 18 followed `4 still, 5 still (1
to 37 125 samples, max 1.0 km/h), 5 moving, 3, 0`. After a pit-lane stop the car waits in the
box with pit mode 2 and leaves with 3, never 5. So the rule: pit mode 5 that has not exceeded 3
km/h since it began = still in the garage; once moving, driving out (latched, also if the car
stops again before the lane). Without the rule, a driver sitting in the car and leaving
produced a junk session: a single out lap at 0 km/h.

### Kerbs and track limits

- Terrain channels are a reliable off-track detector: a lap invalidation on 2026-10-04 matched
  the data (all four wheels on grass, terrain 7, at 873-916 m; the game's flag at 955 m).
- Track limits depend on the place: all four wheels on a kerb happened in 21 of 31 valid laps
  (mostly allowed kerbs). In a race of 2026-10-07, two laps were invalidated about 1.4 s after
  all four wheels went over the kerbs at a corner exit and at a chicane. So kerb-only
  invalidations can only be reported as *possible* causes.
- **Why a lap is not timed or not valid** (what the live frame can say). AMS2 tells: the
  invalid flag (`mLapInvalidated`) with **no cause**; the pit mode; the lap clock at -1 when
  not timed; the session state (formation lap) and the race state (not started, racing...); the
  pit schedule (drive-through and stop-and-go penalties: header only, not seen in game yet). It
  does not tell: a cut, a track-limits warning or penalty as the cause of the flag, nor "black
  and white" warnings in a usable way (not checked). The flag is also set on every lap that is
  not timed: on 2026-10-18 an out lap after a pit stop read `valid: false` with no lap clock.
  The connector therefore sends the lap type (out, in, formation, flying) and a cause it
  **infers** from the terrain before the flag (off track: reliable; kerbs only: probable; else
  unknown).
- The pit lane surface reads as run-off (2026-10-08, 0-33 km/h): pit lane, pits and garage
  samples must not count as off-track.
- On flat kinks the minimum speed sits at the turn-in (the car accelerates through); throttle
  metrics there are not meaningful.

## Game files

- `.vts` setups (in the profile's `savegame\...\vehiclesetups_1_6\`): 2608 B, encrypted in 16 B
  blocks in ECB mode with a **fixed game key** (the same for all users; that is why they can
  be shared by copying the file). No public documentation or tools. **They cannot be read or
  edited**; they can be copied and backed up as opaque files.
- `default.sav` (profile and stats): the same encryption, fixed 7 MiB. **Unreadable.**
- There are no AMS2 leaderboards in the Steam Web API.
- Extracting the key from the executable: ruled out (Steam subscriber agreement; legal grey
  area).
- Automating the menu (AutoHotkey, vJoy): possible but fragile, especially in VR. Ruled out for
  now.
- Plain text and readable: `log\`, `ffb_custom_settings.txt`, the graphics and VR `*.xml`.

## Calibrations and validation results

### First real run (2026-10-04, a 1965 touring car, a US road course, practice, 5 laps)

- Laps 2-5: 1:56.059 / 1:57.492 / 1:57.807 (invalid) / 1:57.997. Official vs measured differ by
  at most 13 ms; S1 + S2 + S3 consistent.
- CPU: **0.6 % of one core** at 50 Hz (a Python recorder, below the 1 % threshold). No VR
  stutter noticed.
- Effective rate about 47.6 Hz (mean dt 21 ms); the only gaps were the per-lap write stalls
  (110-320 ms at lap start), fixed with a background writer.
- Storage: **about 1 MB per lap** (twice the estimate; noisy channels like G and suspension
  velocity compress poorly).
- The setup snapshot captured cold pressures 1.735 / 1.915 bar, brake bias 80 % front, the
  "Vintage" compound.
- Bug found and fixed: the partial lap of a return to the garage via the menu was dropped (its
  duration came from the game lap clock, which resets to -1 in the garage).
- Re-validated on three more sessions (2026-10-04/05): no sampling gap at lap start; the
  partial in-lap saved when returning to the garage via the menu.

### Lap time accuracy

- Measured (game lap clock at the last sample) vs official times: mean +2 ms, max 18 ms over 16
  laps.
- Corner segment times add up to the lap time (116.048 vs 116.059 official on the first
  session).

### Fuel plan calibration

- A 10 min + 1 lap race (2026-10-07): a plan of 7 laps (real: 7) and 28 L; real use to the flag
  24.74 L of 31 loaded.
- A long run (2026-10-04/05): front pressures stable at about 2.15 bar.

### Corner auto-detection

- On a 12-corner road course the auto-detection found 12 corners matching the turns, the esses
  and a chicane. Official maps later fixed the numbering by one.

### Rival timing

- An offline replay of the 2 Hz capture reproduces the in-game picture (player 2:03.409 P1, AI
  2:05.424-2:07.439).
- Recording validated in practice at 50 Hz: 18 cars, 81 and 139 rival laps in two sessions.

### On-track validation of 2026-10-08 (a GT car, two stints)

- Stint change after a pit-lane stop with refuelling (in lap closed at pit exit, fuel at lap end
  taken before the box); out lap and first flying lap separate after both the garage and the
  pit lane (the official time on the first flying lap); the event feed and spoken lap calls
  working in VR.
- The segmentation fix also validated across a race weekend (five sessions, 2026-10-07).

### Lap calls in VR (2026-10-12/13)

- 14 of 14 flying laps called; pressing the radio button before a call made the call wait, no
  overlap.

## Setup screen capture feasibility (2026-10-08)

- With AMS2 in VR, the desktop window (the VR mirror; class `DefaultAppClassName`, title
  "Automobilista 2", client area 1920x1080) shows the setup menu sharp and legible.
- `PrintWindow`, region and desktop grabs all worked, no black frames.
- The setup screen is never byte-identical between frames (ticking session clock, animated 3D
  background): near-duplicate detection needs a tolerance.
- A wheel can show up as **two** devices of the same name with different button counts.
- By voice there are about 10 s between the request and the capture (speech recognition plus
  the agent's turn).
- Validated in VR on 2026-10-13: an agent captured each tab named by the driver and read the
  setup (a rear differential change 4.5:1, 4.0:1, 3.5:1) from the images.

The connector only names the window ([ARCHITECTURE.md](ARCHITECTURE.md#capture-target)); the
host application grabs and stores the capture.

## VR

- If AMS2 runs in native Oculus mode, third-party overlays are not visible, so voice (a
  crew-chief program, the engineer by radio, lap calls) is the most practical channel during
  the session.
- The desktop window mirrors the menus in VR (used by the setup capture); it must not be
  minimized.

## References

- RangeyRover/Automobilista-2-Auto-Director (`shared_memory_struct.py`, CC0);
  viper4gh/CREST2-AMS2; Domaslau/AMS2SharedMemoryNet (C#); CrewChiefV4 (`ams2APIStruct`).
- Architecture model: mishan/assetto-mcp (Assetto Corsa: a 25 Hz recorder to SQLite to MCP
  tools). A similar project: DGBooth/AMS2-Setup-Assistant (Python, uses the Claude API).

## Development gotchas

- On Windows, `Get-ChildItem` can report a stale size (even 0 B) for a file another process
  still has open. Check a file after the writer closes it, or read its content.
- Git Bash on Windows may have no time-zone database: `TZ=Europe/Madrid date` can return UTC.
  Take local time from the Windows API, not from the shell.
