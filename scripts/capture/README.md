# Raw capture of a real session

These three scripts record every frame the game writes to its shared memory, so that
the same real data can be replayed offline (test fixtures, comparisons between implementations).
Run them from PowerShell 5.1 or 7, no admin. They never touch the game or its folder: the
capture only reads the shared memory.

Run them from this folder (`scripts\capture\`):

| Command | What it does |
|---|---|
| `.\start-capture.ps1 -Name practice-imola` | Starts the capture in the background (hidden) and prints the file path. Works before the game is started: it waits for the game. Refuses if a capture is already running. |
| `.\status-capture.ps1` | Says whether a capture runs, the file size, the frames so far, the car, track and session types seen. The numbers are refreshed about every 10 s. |
| `.\stop-capture.ps1` | Stops it cleanly (creates the stop file, waits up to 15 s) and prints the summary. `-Force` ends the process if it does not stop in time. |

`-Name` is a short label for the file name (letters, digits, `-`, `_`).

## Where the files go

`%LOCALAPPDATA%\Stintmate\dev-captures\<yyyyMMdd-HHmmss>_<label>.a2raw`, next to:

* `<file>.a2raw.json`: the summary (frames, duration, first and last timestamp, cars, tracks,
  session types, why it stopped, CPU seconds, size).
* `<file>.a2raw.log`: what the tool printed. `current.json` in the same folder records the
  running capture (pid, stop file); the scripts delete it when the capture is stopped.

The exe is a development build of the recorder: `cargo build --release -p ams2-recorder --features capture-raw`
(this replaces `target\release\ams2-recorder.exe` with a build that has the `capture-raw` subcommand;
the plain release build does not have it). The scripts look for the exe in `%LOCALAPPDATA%\Stintmate\dev-captures\`
(`STINTMATE_CAPTURE_DIR`, when set, replaces that folder for the exe and the files) and then in the repository's `target\release\`; `-Exe <path>` names another.

## A session

1. Before you start the game (or at any time): `.\start-capture.ps1 -Name <car>-<track>`.
2. You drive. `.\status-capture.ps1` shows that frames grow (about 90 to 144 per second
   in VR, 1.8 to 2.8 MB/s, 100 to 170 MB per minute: a long race is several GB, so check the
   free disk space before a long session).
3. When the session is over: `.\stop-capture.ps1`. If the game is closed first, the capture ends
   by itself (reason `game-exited`); a safety cap ends it after 3 hours (`time-limit`).

The capture costs well under 1 % of one core; it runs at below-normal priority.

## What to note

* The file path and the summary that `stop-capture.ps1` printed (frames, capture seconds, cars,
  tracks, session types, stop reason).
* Anything odd: the capture did not start, `status` showed the frames not growing, the stop
  reason is not the expected one, `stop-capture.ps1` needed `-Force`.
* A file cut short (the PC crashed, the process was killed) is still usable: the reader keeps
  every whole frame and warns about the last, partial one.

The files hold only what the game frames hold
(car, track, session state, telemetry), but they are large: do not attach them to a report.
