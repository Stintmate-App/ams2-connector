"""Golden export of the scripted session through the Python tracker (A3 fixture)."""
import json, sys
sys.path.insert(0, r"C:\Users\leona\dev\pitwall\apps\connectors\ams2\tests")
import replay_session as rs
from ams2_connector.recorder import tracker as T
from ams2_connector.recorder import sink as K
from ams2_connector.recorder import names as N, vocab
from ams2_connector.recorder.setup import setup_message
from ams2_connector.messages import (to_params, SessionStart, SessionEnd, PitExit, ParticipantLaps)

T.now_iso = lambda: "ISO"
out = []

def chan_summary(p):
    ch = p.get("channels")
    if ch:
        p["channels"] = {k: {"n": len(v), "first": v[0], "last": v[-1], "sum": sum(v)} for k, v in ch.items()}
    return p

class Rec:
    n = 0
    def start_session(self, info):
        self.n += 1
        car = info.car.strip() or None
        track, layout = info.track.strip() or None, info.variation.strip() or None
        sessions = [N.session_ids(car, info.car_class, track, layout, info.length_m, info.fuel_capacity_l)]
        cat = to_params(N.catalog(sessions))
        p = to_params(SessionStart(started_at=info.started_at, session_type=vocab.session_type(info.session_state),
            car_id=car, track_id=track, layout_id=layout,
            conditions=K._conditions(info.ambient_c, info.track_c, info.rain, info.wind_speed)))
        out.append({"call": "start_session", "catalog": cat, "params": p, "session_state": info.session_state})
        return self.n
    def end_session(self, sid, at):
        out.append({"call": "end_session", "params": to_params(SessionEnd(session_id=0, ended_at=at))})
    def insert_lap(self, rec, rows):
        p = to_params(K.lap_message(rec, rows), channels="json")
        out.append({"call": "insert_lap", "n_rows": len(rows), "params": chan_summary(p)})
    def insert_setup(self, sid, at, reason, snap):
        out.append({"call": "insert_setup", "snapshot": snap, "params": to_params(setup_message(snap, session_id=0, taken_at=at, reason=reason))})
    def insert_pit_exit(self, sid, at, fuel):
        out.append({"call": "insert_pit_exit", "params": to_params(PitExit(session_id=0, at=at, fuel_l=fuel))})
    def insert_participant_laps(self, sid, rows):
        out.append({"call": "insert_participant_laps", "params": to_params(ParticipantLaps(session_id=0, laps=rows))})

class Ev:
    def emit(self, t, **f):
        out.append({"call": "event", "type": t, "fields": f})

script = rs.session_script()
reader_seq = 0; script_seq = None; frames = 0
clock = {"t": 0.0}
sink = Rec()
tr = T.SessionTracker(sink, clock=lambda: clock["t"], events=Ev())
tr.rivals.clock = lambda: clock["t"]
for kind, value in script:
    if kind == "frame":
        if value.mSequenceNumber != script_seq:
            script_seq = value.mSequenceNumber
            reader_seq += 2
        value.mSequenceNumber = reader_seq
        clock["t"] = reader_seq / 2 * rs.DT
        tr.process(value)
    elif kind == "game_exit":
        tr.end_session()
tr.end_session()
json.dump(out, open(sys.argv[1], "w"), indent=0, default=float)
print(len(out), [o["call"] if o["call"]!="event" else o["type"] for o in out])
