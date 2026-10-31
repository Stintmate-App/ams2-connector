# Benchmarks

What the connector costs while it runs. The figures were measured with the tools in this
repository during development (October 2026) on one developer PC (a 3.79 GHz CPU, Windows 11)
that was busy with other work at the time, so tails (maxima, p99) are noisy; the medians were
stable and are the figures to read. They are not re-measured for each release.

## Method

`ams2-measure` (`tools/measure`), release build. A synthetic on-track scenario: the real
recorder loop at 50 Hz in its own process, a test mapping written at 90 frames/s, and a fake
Pitwall that subscribes to `connector.telemetry` in alternate 15 s windows (3 rounds per run).
CPU is `QueryProcessCycleTime` of the recorder process divided by one core's cycles (the clock
is calibrated at start; it read 3.79 GHz). Medians of 3 runs.

```
cargo build --release -p ams2-measure
target\release\ams2-measure.exe --seconds 15 --rounds 3
```

Options: `--seconds` (default 20), `--rounds` (3), `--fps` (frames written per second, 90),
`--cars` (participants in the test mapping, 30).

## Recorder

| Metric | Median of 3 runs |
|---|---|
| Recorder CPU at 50 Hz, no subscription | 0.123 % of one core |
| Recorder CPU while the telemetry stream runs at 90 frames/s | 0.490 % |
| What the stream adds | 0.363 % |
| Allocations on the loop per tick, steady state (status ticks included; `bins/ams2-recorder/tests/alloc.rs`) | 0 |
| Loop period p99 | 20.6 ms (the period is 20 ms) |
| Stream `t_ns` after the frame was written, median | 0.53 ms |
| Tick median (snapshot, tracker, detector, live-frame publish; with the stage probes) | 9.6-10.0 us |
| A 90 s lap encoded and stored (4500 samples of 72 channels, 1.30 MB; `lap_bytes` test) | 4.7-5.0 ms |
| Start to first snapshot, inside the process (build, links, mapping, first tick; process creation not included) | 2 ms |
| Recorder private working set | 7.3 MB |

Tick stages (median of the ticks that saw a frame): snapshot 2.2 us, tracker 4.1 us, live-frame
publish 1.2 us, detector and status 1.2 us.

The stream was split once with a temporary switch that skipped only `StreamChannel::send` (not
kept): CPU with the subscription fell from 0.494 % to 0.237 %, so the sampling thread adds
about 0.09 % and the IPC send about 0.27 % (a `Bytes` copy, the subscriber lock and one
`WriteFile` per sample, about 25 us each).

The lap-write timing is `cargo test --release -p ams2-recorder --test lap_write -- --ignored --nocapture`.

## Control app

Tray process idle with the window hidden, recorder not running, 60 s, measured by the
`tray_probe` example of `ams2-connector-ui` (`cargo run --release --example tray_probe`):
0.021 % of one core (three runs: 0.023, 0.021, 0.021).

## Spotter

`ams2-golden bench` (`tools/golden`) on a race capture of 43,455 ticks (not included in the
repository), release build: the spotter alone per frame, median 0.10 us (the timer's
resolution), p99 0.10 us, p99.9 0.20 us, max 2.5 us; the whole hot path at full speed, median
1.50 us; paced at 50 Hz, median 9.2 us, 0.154 % of a core. The spotter made 73 calls on that
capture (42 side changes and 31 "still there" calls).

`ams2-measure --seconds 15 --rounds 2` with 30 participants around the player and two
alongside against one participant: tick median 11.5 us against 10.7 us (the same run), p99
69 us against 52 us. `tests/alloc.rs` runs the spotter's scan (a passing car beside the player)
on every tick: 0 allocations per steady-state tick. A call allocates (one `Announcement`), so a
tick with a state change does.

## Executables

`cargo build --release --locked` (`lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`,
unwinding kept), version 3.2.2, sizes of the files built on Windows 11:

| Executable | Size |
|---|---|
| `ams2-recorder.exe` | 3.3 MB (3,463,168 bytes) |
| `ams2-connector-ui.exe` | 3.6 MB (3,822,592 bytes) |
| `ams2-connector-window.exe` | 8.8 MB (9,255,424 bytes; egui and the glow renderer, fonts included) |

An earlier build (October 2026, before the spotter) measured a `--version` start of 11.5 ms
(recorder), 12.0 ms (control app) and 77.7 ms (window), median of 10 runs of
`Start-Process -Wait`, which includes the launch.

## What the figures do not cover

Virtual reality, the real game, the cost of a long session, and the shipped
`ams2-recorder.exe` running against the game (the measurements use the same library code in the
`ams2-measure` process).
