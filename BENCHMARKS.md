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
