"""Generates the parity fixtures of ams2-core from an earlier Python implementation of the connector.
That implementation is not in this repository: this script is the record of how the fixtures were
made and cannot be run from a clean clone.

Run: uv run --no-project crates/ams2-core/scripts/gen_announce_fixtures.py [PYTHON_APP_DIR]
(PYTHON_APP_DIR is the folder of that implementation). Needs nothing installed:
the Python modules are imported with stubs for stintmate_ipc and connector_messages.

Writes, next to this script's crate:
  data/catalog.json                      the announcement catalog (embedded by ams2-core)
  fixtures/announce_parity.json          sha256-12 keys and hashes, actions, guidance
  fixtures/live_frame_cases.json.gz      307 snapshot specs + Pitwall's expected frames
  fixtures/*.schema.json                 copies of the protocol and write API schemas
  fixtures/detector_replay.json.gz       a seeded random replay and what the earlier implementation announced
"""
from __future__ import annotations

import gzip
import json
import random
import os
import shutil
import sys
import types
from pathlib import Path

CRATE = Path(__file__).resolve().parent.parent
APP = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(
    os.environ.get("PYTHON_CONNECTOR_DIR", "."))  # the earlier implementation's checkout
sys.path[:0] = [str(APP / "src"), str(APP / "tests")]

ipc = types.ModuleType("stintmate_ipc")
ipc.RemoteError = type("RemoteError", (Exception,), {})
sys.modules["stintmate_ipc"] = ipc
cm = types.ModuleType("ams2_connector.connector_messages")
cm.parse_params = lambda *a, **k: {}
sys.modules["ams2_connector.connector_messages"] = cm

from ams2_connector import actions, guidance  # noqa: E402
from ams2_connector.announce import catalog as C  # noqa: E402
from ams2_connector.announce.detector import EventDetector  # noqa: E402
from ams2_connector.announce.sender import catalog_ik  # noqa: E402
from ams2_connector.shm.layout import SharedMemory, decode_str  # noqa: E402
import live_cases  # noqa: E402


def dump(path: Path, obj, gz=False, indent=None):
    text = json.dumps(obj, ensure_ascii=False, indent=indent,
                      separators=(",", ":") if indent is None else None)
    if gz:
        with open(path, "wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as f:
            f.write(text.encode("utf-8"))
    else:
        path.write_text(text + "\n", encoding="utf-8")


# ---- catalog, actions, guidance ---------------------------------------------------------
dump(CRATE / "data" / "catalog.json", C.CATALOG, indent=1)
dump(CRATE / "fixtures" / "announce_parity.json", {
    "catalog_ik": catalog_ik(),
    "source_hashes": {code: C.source_hash(m) for code, m in C.MESSAGES.items()},
    "validation": C.VALIDATION,
    "actions_ik": actions.actions_ik(),
    "actions": actions.ACTIONS,
    "guidance_ik": guidance.items_ik(),
    "guidance": guidance.ITEMS,
}, indent=1)
for schema in ("connector-protocol", "write-api"):
    shutil.copyfile(APP / "tests" / "fixtures" / "schemas" / f"{schema}.schema.json",
                    CRATE / "fixtures" / f"{schema}.schema.json")

# ---- live frame cases -------------------------------------------------------------------
SCALARS = {  # python name -> rust name
    "mGameState": "game_state", "mSessionState": "session_state", "mRaceState": "race_state",
    "mViewedParticipantIndex": "viewed_participant_index", "mNumParticipants": "num_participants",
    "mLapsInEvent": "laps_in_event", "mTrackLength": "track_length",
    "mLapInvalidated": "lap_invalidated", "mBestLapTime": "best_lap_time",
    "mLastLapTime": "last_lap_time", "mCurrentTime": "current_time",
    "mSplitTimeAhead": "split_time_ahead", "mSplitTimeBehind": "split_time_behind",
    "mEventTimeRemaining": "event_time_remaining", "mPitMode": "pit_mode",
    "mPitSchedule": "pit_schedule", "mFuelLevel": "fuel_level", "mFuelCapacity": "fuel_capacity",
    "mSpeed": "speed", "mRpm": "rpm", "mGear": "gear", "mAmbientTemperature": "ambient_temperature",
    "mTrackTemperature": "track_temperature", "mRainDensity": "rain_density",
    "mWindSpeed": "wind_speed", "mSessionDuration": "session_duration",
    "mSessionAdditionalLaps": "session_additional_laps",
}
TEXTS = {"mCarName": "car_name", "mCarClassName": "car_class_name",
         "mTrackLocation": "track_location", "mTrackVariation": "track_variation"}
WHEEL = {"mTyreTempLeft": "tyre_temp_left", "mTyreTempRight": "tyre_temp_right",
         "mTyreTempCenter": "tyre_temp_center", "mTyreCarcassTemp": "tyre_carcass_temp",
         "mTyreWear": "tyre_wear", "mBrakeTempCelsius": "brake_temp_celsius",
         "mAirPressure": "air_pressure"}
PART = {"mCurrentLapDistance": "current_lap_distance", "mRacePosition": "race_position",
        "mLapsCompleted": "laps_completed", "mCurrentLap": "current_lap",
        "mCurrentSector": "current_sector", "mIsActive": "is_active"}
PER_CAR = {"mPitModes": "pit_modes", "mLastLapTimes": "last_lap_times",
           "mFastestLapTimes": "fastest_lap_times"}


def spec(d: SharedMemory) -> dict:
    out = {"scalars": {r: float(getattr(d, p)) for p, r in SCALARS.items()},
           "texts": {r: decode_str(bytes(getattr(d, p))) for p, r in TEXTS.items()},
           "wheels": {r: [float(x) for x in getattr(d, p)] for p, r in WHEEL.items()}}
    n = d.mNumParticipants
    parts = []
    for i in range(max(0, min(n, 64))):
        p = d.mParticipantInfo[i]
        row = {r: float(getattr(p, k)) for k, r in PART.items()}
        row["name"] = decode_str(p.mName)
        row["car_name"] = decode_str(bytes(d.mCarNames[i]))
        row["class_name"] = decode_str(bytes(d.mCarClassNames[i]))
        for k, r in PER_CAR.items():
            row[r] = float(getattr(d, k)[i])
        parts.append(row)
    out["participants"] = parts
    return out


frames = live_cases.fixture()["frames"]
cases = [spec(d) for d in live_cases.cases()]
assert len(cases) == len(frames) == 307
dump(CRATE / "fixtures" / "live_frame_cases.json.gz", {"cases": cases, "frames": frames}, gz=True)

# ---- detector replay --------------------------------------------------------------------
LENGTH = 4000.0
HZ = 50
rng = random.Random(52)


def race(n=8):
    d = SharedMemory()
    d.mVersion = 14
    d.mGameState, d.mSessionState, d.mRaceState = 2, 5, 2
    d.mTrackLength, d.mLapsInEvent, d.mSessionDuration = LENGTH, 6, 0.0
    d.mViewedParticipantIndex, d.mNumParticipants = 0, n
    d.mHighestFlagColour = 1
    d.mSpeed = 50.0
    for i in range(n):
        p = d.mParticipantInfo[i]
        p.mIsActive = True
        p.mName = b"Me" if i == 0 else f"Rival {i}".encode()
        p.mRacePosition = i + 1 if i else 3
        p.mCurrentLap, p.mLapsCompleted = 3, 2
        p.mCurrentLapDistance = 1000.0 + 100 * i
        p.mCurrentSector = 1
        d.mCarClassNames[i].value = b"GT3"
        d.mRaceStates[i] = 2
    d.mParticipantInfo[1].mRacePosition, d.mParticipantInfo[2].mRacePosition = 1, 2
    return d


SCALAR_ATTR = {
    "highest_flag_colour": "mHighestFlagColour", "highest_flag_reason": "mHighestFlagReason",
    "yellow_flag_state": "mYellowFlagState", "pit_mode": "mPitMode",
    "pit_schedule": "mPitSchedule", "speed": "mSpeed", "race_state": "mRaceState",
    "game_state": "mGameState", "session_state": "mSessionState",
    "viewed_participant_index": "mViewedParticipantIndex", "laps_in_event": "mLapsInEvent",
    "session_duration": "mSessionDuration"}
MUT = [
    ("highest_flag_colour", [0, 1, 1, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
    ("highest_flag_reason", [0, 1, 2, 3]),
    ("yellow_flag_state", [-1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
    ("pit_mode", [0, 0, 0, 1, 2, 3]),
    ("pit_schedule", [0, 0, 5, 6, 7]),
    ("speed", [0.0, 0.5, 30.0, 50.0]),
    ("race_state", [1, 2, 2, 2, 3, 4, 5, 6, 0]),
    ("game_state", [2] * 14 + [3, 1, 4, 5]),
    ("session_state", [5] * 14 + [1, 3, 4]),
    ("viewed_participant_index", [0, 0, 0, 2, 9]),
    ("race_states", [(i, v) for i in range(1, 8) for v in (2, 2, 3, 4, 5, 6)]),
    ("lap", [(i, v) for i in range(0, 8) for v in (3, 4, 5, 6)]),
    ("dist", [(i, v) for i in range(0, 8) for v in (500.0, 900.0, 1000.0, 3900.0)]),
    ("is_active", [(i, v) for i in range(1, 8) for v in (0, 1, 1)]),
    ("rename", [(i, nm) for i in range(1, 8)
                for nm in ("Rival 1", "Rival 5", "Safety Car", "Me", "Zed")]),
    ("laps_in_event", [0, 6, 6, 10]),
    ("session_duration", [0.0, 0.0, 30.0]),
    ("sector", [0, 1, 2]),
]


def apply(d, name, v):
    if name in SCALAR_ATTR:
        setattr(d, SCALAR_ATTR[name], v)
    elif name == "race_states":
        d.mRaceStates[v[0]] = v[1]
    elif name == "lap":
        d.mParticipantInfo[v[0]].mCurrentLap = v[1]
    elif name == "dist":
        d.mParticipantInfo[v[0]].mCurrentLapDistance = v[1]
    elif name == "is_active":
        d.mParticipantInfo[v[0]].mIsActive = bool(v[1])
    elif name == "rename":
        d.mParticipantInfo[v[0]].mName = v[1].encode()
    elif name == "sector":
        d.mParticipantInfo[0].mCurrentSector = v


class Clock:
    t = 1000.0

    def __call__(self):
        return self.t


clock = Clock()
det = EventDetector(clock=clock)
d = race()
steps, events = [], []
TICKS = 150000
for tick in range(TICKS):
    muts = []
    if tick % 75 == 0 and rng.random() < 0.8:
        for _ in range(rng.choice([1, 1, 1, 2])):
            name, choices = rng.choice(MUT)
            v = rng.choice(choices)
            muts.append([name, list(v) if isinstance(v, tuple) else v])
            apply(d, name, v)
    if tick % 3000 == 2999:  # a reset now and then
        muts.append(["reset", 0])
        det.reset()
    steps.append(muts)
    clock.t += 1.0 / HZ
    d.mSequenceNumber += 2
    for a in det.update(d):
        events.append({"tick": tick, "code": a.code, "params": a.params, "data": a.data})
print("replay events:", len(events), sorted({e["code"] for e in events}))
dump(CRATE / "fixtures" / "detector_replay.json.gz", {
    "steps": [[i, m] for i, m in enumerate(steps) if m], "ticks": TICKS, "events": events},
    gz=True)
print("ok")
