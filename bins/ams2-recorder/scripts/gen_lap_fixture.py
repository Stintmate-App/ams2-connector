"""Byte-level lap fixture, made once with an earlier Python implementation of the connector that is
not in this repository (this script is kept as the record of how `tests/fixtures/lap_params_300.msgpack`
was made; it needs that implementation's classes and cannot be run from a clean clone).

Usage: uv run --directory <folder of that implementation> --no-sync python gen_lap_fixture.py <out_dir> [rows]

Writes <out_dir>/lap_params_<rows>.msgpack: msgpack.packb(to_params(lap_message(record, rows)),
use_bin_type=True) for a deterministic record and rows, exactly the `p` of the outbox file.
Also prints the size of a 4500-row (90 s) lap file the way the outbox writes it.
"""
import sys
from pathlib import Path

import msgpack

from ams2_connector.messages import to_params
from ams2_connector.recorder import channels as C
from ams2_connector.recorder import sink as K


def rows_for(n):
    # Same formula in the Rust test: exact in f64, not exact in f32.
    return [[((i * 37 + j * 11) % 1013) * 0.173 for j in range(len(C.CHANNEL_NAMES))]
            for i in range(n)]


def record(n):
    return K.LapRecord(
        session_id=1, lap_number=3, started_at="2026-10-23T10:00:00.000+02:00",
        lap_time_s=90.123, measured_time_s=90.1, sector1_s=30.1, sector2_s=29.9, sector3_s=30.123,
        complete=True, valid=True, out_lap=False, in_lap=False, fuel_start_l=50.0, fuel_end_l=47.5,
        max_speed_kmh=250.5, n_samples=n, on_track=True, game_lap=3, lap_time_source="official",
        formation_lap=False, ambient_c=21.0, track_c=33.5, rain=0.0, wind_speed=2.5)


def params(n):
    p = to_params(K.lap_message(record(n), rows_for(n)))
    p["session_key"] = "0123456789abcdef"
    p["seq"] = 3
    return p


if __name__ == "__main__":
    out = Path(sys.argv[1])
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 300
    out.mkdir(parents=True, exist_ok=True)
    (out / f"lap_params_{n}.msgpack").write_bytes(msgpack.packb(params(n), use_bin_type=True))
    big = msgpack.packb({"method": "lap", "v": 1, "ik": "x", "p": params(4500),
                         "at": "2026-10-23T10:00:00.000+02:00"}, use_bin_type=True)
    print("90 s lap file bytes:", len(big))
