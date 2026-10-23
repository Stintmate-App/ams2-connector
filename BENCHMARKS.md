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
