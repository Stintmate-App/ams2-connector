# Benchmarks

`ams2-measure` (`tools/measure`), release build, synthetic on-track scenario of the Python
`measure_telemetry.py`: the real recorder loop at 50 Hz in its own process, a test mapping
written at 90 frames/s, a fake Pitwall that subscribes to `connector.telemetry` in alternate
15 s windows (3 rounds per run). CPU is `QueryProcessCycleTime` of the recorder process divided
by one core's cycles (3.79 GHz, calibrated). Run on the developer PC 2026-10-23 **while it was
busy** (other sessions and apps running), so tails are noisy; the medians were stable.

```
cargo build --release -p ams2-measure
target\release\ams2-measure.exe --seconds 15 --rounds 3
```

## B2 (2026-10-23), median of 3 runs

| Metric | Target (plan 3) | Run 1 | Run 2 | Run 3 | Median | Met |
|---|---|---|---|---|---|---|
| Recorder CPU at 50 Hz, no subscription | <= 0.2 % | 0.123 | 0.127 | 0.122 | 0.123 % | yes |
| Recorder CPU with the stream at 90 frames/s | <= 0.4 % | 0.489 | 0.462 | 0.469 | 0.469 % | **no** |
| The stream adds | <= 0.2 % | 0.365 | 0.335 | 0.348 | 0.348 % | **no** (Python: 0.44) |
| Loop period p99 | <= 20.5 ms | 21.31 | 20.64 | 20.65 | 20.65 ms | within 0.15 ms |
| Loop period max | <= 21 ms | 37.10 | 21.67 | 20.95 | 21.67 ms | run 1 hit by a busy PC |
| Stream sample period sd | <= 0.5 ms | 0.57 | 0.57 | 0.55 | 0.57 ms | **no** (Python: 0.54) |
| Stream sample period p99 | <= 12.5 ms | 12.74 | 12.73 | 12.72 | 12.73 ms | **no** (Python: 12.5) |
| `t_ns` after the frame was written (median / p99) | <= 0.5 ms | 1.02 / 2.03 | 1.02 / 2.06 | 1.02 / 2.06 | 1.02 ms | **no** |
| Tick (snapshot + tracker + detector + live publish) median | <= 5 us | 9.3 | 8.8 | 8.4 | 8.8 us | **no** |
| Recorder private working set | <= 15 MB | 7.3 | 7.3 | 7.2 | 7.3 MB | yes |

Stream rate: 88.8 samples/s at 90 frames/s (every frame, none lost).

Not measured here: allocations per tick (needs the test-allocator build), the encode and store
time of a 90 s lap (B1's writer), control app figures (U tasks), start to first snapshot, VR.

### Reading the misses

* The stream's extra CPU is mostly the wake-ups (1.25 per frame, a waitable-timer sleep each)
  and the IPC publish of every sample (`Bytes` copy, subscriber lock, the I/O thread's pipe
  write, all in this process). A cheaper cadence (coalescing the idle polls, a spin-then-sleep
  at the end of the wait) and a measured split by thread are the next step.
* `t_ns` lag of 1 ms: the thread wakes `LATE` (0.3 ms) after the expected frame and then polls
  every 1 ms until the sequence number moves; the Python version has the same design and
  quotes 0.5 ms for its own clock. Tightening `DUE_POLL` costs CPU, so it trades against the
  line above.
* Tick median: two 20 kB copies (the snapshot and the live-frame publish) plus the tracker.
  The publish could be skipped while nobody asked for a live frame lately (the first request
  would then wait for the next frame, up to 20 ms).

## Tuning (2026-10-23), median of 3 runs, same method

Same PC, still busy (other sessions and builds running): the baseline was re-measured just
before the changes with the B2 binary (`stintmate-ipc` at `f45be37`), and two of its three runs
were hit hard (loop max 72 ms, CPU with the stream 0.767 %); the "after" runs were calmer. CPU
figures within about 0.03 % are noise. The tray is measured by `tray_probe` (now
`QueryProcessCycleTime`, exact; the 15.6 ms-sampled figure is printed too), 60 s idle, window
hidden, recorder not installed, three runs each.

| Metric | Target | Before (today) | After | Met |
|---|---|---|---|---|
| Tray process idle, window hidden | <= 0.05 % | 0.152 / 0.159 / 0.151 (0.152 %) | 0.023 / 0.021 / 0.021 (0.021 %) | yes |
| Recorder CPU at 50 Hz, no subscription | <= 0.2 % | 0.121 / 0.170 / 0.133 | 0.127 / 0.129 / 0.121 (0.127 %) | yes |
| Recorder CPU with the stream at 90 frames/s | <= 0.4 % | 0.465 / 0.767 / 0.560 | 0.482 / 0.451 / 0.464 (0.464 %) | **no** |
| The stream adds | <= 0.2 % | 0.344 / 0.597 / 0.427 | 0.355 / 0.321 / 0.343 (0.343 %) | **no** |
| Loop period p99 / max | <= 20.5 / 21 ms | 21.36-43.5 / 37-72 | 20.63, 22.01, 20.65 / 20.84, 116, 126 | p99 within 0.15 ms; max: busy-PC outliers |
| Stream period sd / p99 | <= 0.5 / 12.5 ms | 0.98-4.77 / 12.8-33.6 | 0.40, 1.93, 1.39 / 12.09, 14.17, 12.19 | p99 yes; sd only in the calm run |
| `t_ns` after the frame (median) | <= 0.5 ms | 1.02 / 1.51 / 1.50 | 0.533 / 0.537 / 0.538 | within 0.04 ms |
| Tick median (with the stage probes) | <= 5 us | 9.4 / 15.4 / 12.7 | 9.6 / 9.8 / 9.4 | **no** |
| Allocations on the loop, steady state | 0 per tick | not measured | 0 on 23 of 25 ticks; see below | **no** |
| A 90 s lap encoded + stored | <= 5 ms | not measured | 4.7-5.0 ms (1.30 MB file, 4500 samples; `lap_bytes` test) | yes (marginal) |
| Start to first snapshot (in process) | <= 0.3 s | not measured | 1.8 / 1.8 / 2.0 ms | yes |

Tick stages (QPC probes, median of the ticks that saw a frame): snapshot 2.2 us, tracker
4.1 us, live-frame publish 1.2 us, detector + status 1.2 us.

### What changed

* Control app: the supervisor no longer ticks every second for nothing. It ticks every 1 s
  only while a service runs, is wanted, stopping or restarting; every 2 s in managed mode
  (the parent check); else every 5 s, sleeping on a condition variable that commands and
  `service.status` updates wake. `ui.snapshot` / `control.state` were already pushed only on
  change; now the tray is told only on change too. The autostart value is cached for 30 s and
  refreshed at once after the app's own change.
* The game check (`ams2_platform::game::GameProbe`): the process snapshot cost 6.1-6.4 ms on
  this PC, every 5 s, which was nearly all of the tray's 0.15 %. Now: a zero-timeout wait on
  the game's process handle while it runs; else a `FindWindowW` by the game's class and title,
  with the snapshot only when such a window appears, or every 60 s as a backstop. The
  recorder's `GameWatch` uses it too.
* Recorder: below-normal priority set by its own `main` (the control app already started it
  so); the loop and the telemetry thread one step above their process's class. The telemetry
  thread polls every 0.25 ms once a frame is due (was 1 ms): the `t_ns` lag halves, CPU
  unchanged within noise.
* Measuring: per-stage QPC probes in the tick, start-to-first-snapshot, the counting-allocator
  test (`bins/ams2-recorder/tests/alloc.rs`) and the lap-write timing
  (`tests/lap_write.rs`, ignored by default:
  `cargo test --release -p ams2-recorder --test lap_write -- --ignored --nocapture`).

### Still missed, and why

* Allocations: the 0.5 s status builds its two JSON values (about 245 allocations) to compare
  with the last ones sent, and the rivals' 2 Hz batch allocates about 11. Every other tick
  allocates nothing (the lap's rows are reserved). Getting to 0 means building the status
  from a small fixed struct compared field by field, or on another thread; not done here.
  The test asserts at most 2 allocating ticks in 25.
* The stream's CPU: the extra wake-ups were not the cost (a 4x denser due-poll left it
  unchanged); what remains is the per-sample publish (copy, subscriber lock, the I/O thread's
  pipe write). Needs a per-thread split next.
* Tick median: the tracker (4.1 us) and the 20 kB snapshot copy (2.2 us) dominate; skipping
  the live-frame publish while nobody asks would save 1.2 us, not enough on its own.
* Lap encode + store 4.7-5.0 ms for a 1,298,613 byte file (message, value, encode, write, fsync,
  rename; Python: 1.30 MB in 12 + 2.1 ms). The earlier 2.59 MB / 8.7 ms came from the
  `lap_write` test, whose synthetic frames never reached the line properly: the saved "lap"
  held 9000 samples (two laps). The encoding is byte-equal to Python's (`lap_bytes` test).
* Start to first snapshot is measured inside the process (build, links, mapping, first tick);
  process creation is not included.

## Release build (2026-10-23, PRY-057 P1)

`cargo build --release --locked` (`lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`,
unwinding kept), the PC busy as above.

| Executable | Size | `--version` start (median of 10, `Start-Process -Wait`, includes the launch) |
|---|---|---|
| `ams2-recorder.exe` | 3.2 MB (3,354,624 bytes) | 11.5 ms |
| `ams2-connector-ui.exe` | 3.6 MB (3,743,232 bytes) | 12.0 ms |
| `ams2-connector-window.exe` | 8.2 MB (8,586,240 bytes; egui + glow) | 77.7 ms |

* Recorder idle memory: `ams2-measure --seconds 10 --rounds 1` (the recorder loop in its own
  process, the same library code as the exe): private working set **5.1 MB**, working set
  9.7 MB (B2: 7.3 MB private; target <= 15 MB). The shipped `ams2-recorder.exe` itself was not
  started: it has no override for its single-instance names and the driver's connector runs on
  this PC with the production ones.
* `capture-raw` is absent: the feature is off by default and no crate of the workspace enables
  it (`cargo tree -e features --workspace` has no `capture-raw`), and the string of its code
  (`capture-raw needs a FILE`) is not in `ams2-recorder.exe`, while the stub's
  (`is not available in this build`) is.

## Status as a fixed struct (2026-10-25), median of 3 runs, same method

Same scenario (`ams2-measure --seconds 15 --rounds 3`), the PC busy; the "before" runs are
`main` at `98a9bfe`, the "after" runs this branch. The clock calibration read 3.79 GHz in
both. Figures within about 0.03 % are noise.

| Metric | Target | Before (3 runs) | After (3 runs) | Met |
|---|---|---|---|---|
| Recorder CPU at 50 Hz, no subscription | <= 0.2 % | 0.133 / 0.126 / 0.262 (0.133 %) | 0.130 / 0.119 / 0.123 (0.123 %) | yes |
| Recorder CPU with the stream at 90 frames/s | <= 0.4 % | 0.475 / 0.475 / 0.965 (0.475 %) | 0.490 / 0.481 / 0.494 (0.490 %) | **no** |
| The stream adds | <= 0.2 % | 0.342 / 0.349 / 0.703 (0.349 %) | 0.360 / 0.363 / 0.371 (0.363 %) | **no** |
| Tick median (with the stage probes) | <= 5 us | 10.4 / 9.6 / 17.1 | 9.6 / 9.8 / 10.0 | **no** |
| Allocations on the loop, steady state | 0 per tick | 0 on 23 of 25 ticks | 0 on every tick (status ticks included; `tests/alloc.rs`) | yes |

The status: `StatusInput` (`status.rs`) is filled in place on the loop (borrowed views of the
tracker, the sink, the outbox and the link instead of cloned values), compared with the last
one handed over and, only if it differs, copied into a slot read by a publisher thread that
builds the JSON and sets the channels. The JSON is unchanged. `ams2-golden export` + `compare`
on `20261025-212520_practice.a2raw` against the committed golden: 0 differences (66 calls,
4 announcements, 182 live frames).

The stream, split once with a temporary switch that skipped only `StreamChannel::send` (not
kept): CPU with the subscription 0.237 % against 0.494 % with the send, so the sampling thread
adds about 0.09 % and the IPC send about 0.27 % (a `Bytes` copy, the subscriber lock and one
`WriteFile` per sample, about 25 us each). The connector cannot lower that without changing
the samples it sends or when; the remaining lever is `stintmate-ipc` writing the queued
samples of a subscription in one call. Batching in the connector would only delay samples,
which the brake cues cannot afford, and still make one write each.

## Spotter (2026-10-27, PRY-098)

`ams2-golden bench` on the race capture 20261025-215838 (43,455 ticks, release build): the spotter alone
per frame, median 0.10 us (the timer's resolution), p99 0.10 us, p99.9 0.20 us, max 2.5 us; the whole hot
path at full speed median 1.50 us, paced at 50 Hz median 9.2 us, 0.154 % of a core. The spotter says 73
calls on the capture (42 side changes and 31 "Still there").

`ams2-measure --seconds 15 --rounds 2` (30 participants around the player, two alongside, against 1 as
before; the `--cars` switch): tick median 11.5 us against 10.7 us (the same run on this PC; the tracker's
stage also grows 0.7 us with the extra participants, the detector + status stage 0.1 us), p99 69 us against
52 us. The test `bins/ams2-recorder/tests/alloc.rs` runs the spotter's scan (a passing car beside the
player) on every tick: 0 allocations per steady-state tick. A call allocates (one `Announcement`), so a
tick with a state change does.
