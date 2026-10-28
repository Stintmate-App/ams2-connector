# AMS2 connector: architecture

How the connector is built and why. How to run it: [usage.md](usage.md). The AMS2 facts it
relies on, with their evidence: [FINDINGS.md](FINDINGS.md). What it speaks to Pitwall and to a
managing app is the Stintmate application contract over
[Stintmate IPC v2](https://github.com/Stintmate-App/stintmate-ipc) and the connector protocol
(`connector-protocol.md` in the Stintmate repository); this page names the methods and
channels but does not repeat the protocol. For a reader who wants to build another connector:
[writing-a-connector.md](writing-a-connector.md).

## Processes

Three executables, in one folder, all written in Rust:

```
ams2-connector-ui.exe (tray, Local\Ams2Connector.UI)        pitwall-core (Pitwall)
  serves connector-ams2: control.*, control.state,            serves pitwall
  ui.snapshot, control.notices; subscribes service.status        ^
  |- starts ams2-connector-window.exe on demand (egui)           |
  `- starts ams2-recorder.exe -- client of pitwall ---------------+ pitwall.write.* (outbox),
                                 + of connector-ams2              | pitwall.catalog.declare,
                                   (service.status)               | pitwall.announce; declares
                                                                  | connector.status, .live_frame,
        ^ read-only                                               | .capture_target, .telemetry,
  AMS2: shared memory $pcars2$ (recorder), its desktop window     | .action
```

| Process | Exe | Role | Named objects (`Local\`) |
|---|---|---|---|
| Control app (the tray process) | `ams2-connector-ui.exe`, GUI subsystem, no window | tray, supervises the recorder, serves `connector-ams2`, descriptor, autostart, managed mode | `Ams2Connector.UI` |
| Window | `ams2-connector-window.exe`, GUI subsystem | the Services, Options and Logs pages; started on demand, gone when closed | one instance at most |
| Recorder | `ams2-recorder.exe`, console subsystem, started hidden | shared memory, the 50 Hz loop, the tracker, the outbox, the `pitwall` link | `Ams2Connector.Recorder` (+ `.Stop`) |

Pitwall sees one connector (`peer.app` `connector-ams2`, `peer.meta` with `connector_id`
`ams2`, `connector_name`, `sim`, `sim_name`, `service`) whose functions are the union of what
its connected processes declare. A method whose process is off answers
`not_implemented_by_connector`.

**Why separate processes.** A GUI toolkit, a graphics context and a message loop must never
share a process with the 50 Hz loop: a UI crash or hang must not lose a lap, and the recorder
runs at below-normal priority with no GUI library in its address space. The window is a third
process, not a part of the tray, because its toolkit always creates a graphics context while it
exists (measured: `docs/ui-spike.md`: an OpenGL context and about 120 MB for an idle hidden
window). The tray process, which is always running, therefore has no graphics context at all,
and the window exists only while the user looks at it. The window talks to the control app
over the same endpoint a managing app uses: it receives a `ui.snapshot` and sends `ui.command`;
a second "Open" while it runs only publishes a `window_request` in the snapshot, which the
window watches. A crash of the window loses nothing.

**The window's layout (PRY-087).** It opens at 680x460 logical px (minimum 560x380), resizable,
always dark. A side menu of three items with painted icons (the selected one filled) and "Version
x.y.z" at the bottom, then one page: **Services** (a card per service: name, a status pill coloured
by state, the switch with its state in words, tiles for the Pitwall link, the CPU and the sampling
rate, one sentence about the game; a data folder card with **Open**; a footer line when a managing
app runs it), **Options** (cards of settings rows, each switch with its note and state in words,
then the sampling rate) and **Logs** (a Recorder | App segmented control, "Only warnings and
errors", Copy last 50, Open log file, and a monospace list of time, level and message with the full
line as a tooltip; warnings amber, errors red). Every text is a key of the built-in catalog; the
palette is a set of constants in `theme.rs`, which also paints the switch, the pill and the menu
icons. The fonts are embedded in the exe (about 0.5 MB): Barlow (interface), Barlow Condensed
ExtraBold (page titles) and JetBrains Mono (logs and figures), all SIL OFL 1.1, with their licence
texts in `bins/ams2-connector-window/assets/fonts/` and in `THIRD-PARTY-NOTICES.txt`. The window adds
no GPU or background work: a repaint at most once a second, the log list draws only its visible rows
(a tail of 500 lines), and the recorder's CPU use is two `GetProcessTimes` readings a second apart,
taken only while the Services page is open. `tests/render.rs` draws each page offscreen
(egui's headless context and a small CPU rasteriser) into `target/tmp/pry087-*.png`.

## Crates

```
crates/
  ams2-shm/       the #[repr(C)] layout of SharedMemory v14 with compile-time size and offset
                  asserts; Mapping (OpenFileMappingW, MapViewOfFile); the seqlock snapshot
                  into a preallocated buffer; a test mapping writer; the raw capture format
  ams2-core/      pure logic: no Windows, no IPC, injected clock. Tracker (sessions, laps,
                  pit exits), garage rule, rivals, channels (72, units, signs), vocab, setup,
                  names, messages (the write params), live frame, telemetry sample mapping,
                  announce (catalog, detector, rate window), actions and guidance as data
  ams2-platform/  Windows helpers shared by the executables: workspace resolver, data folder,
                  logging, named mutex and stop event, process helpers (game check, job
                  object, spawn suspended), high-resolution timer
  ams2-i18n/      the text catalogs contract loader (chain, validation, plurals)
bins/
  ams2-recorder/            the recorder, its links, outbox and subcommands
  ams2-connector-ui/        the control app: endpoint, ServiceManager, tray, config, descriptor
  ams2-connector-window/    the egui window
tools/
  measure/        what the recorder costs (CPU, jitter, working set), see Performance
  replay/         plays a scripted session or a raw capture into a test mapping
```

The split follows what must be testable without the game and without a pipe: everything that
decides what a lap is lives in `ams2-core`, takes frames in and gives records out, and is run
by unit tests, property tests and golden comparisons against the earlier implementation. The
executables add the clock, the threads, the disk and the pipes.

## Threads of the recorder

| Thread | Does | Never |
|---|---|---|
| main (the loop) | at 50 Hz: snapshot, tracker, detector; status every 0.5 s; hands laps to the writer by channel | allocates per tick, writes to disk, waits on IPC |
| writer | encodes a lap or session message and writes the outbox file | |
| outbox sender | sends files in order, one at a time, waits for answers, retries with the same key | |
| telemetry | only while Pitwall subscribes: its own view of the mapping, phase-locked sampling, publishes into the stream | touches the loop's buffers |
| IPC I/O threads and a handler pool | the links; `connector.live_frame` answered inline from the last snapshot; `connector.capture_target` and `connector.action` on the pool | block the loop |

The live frame needs the last snapshot on the request thread: the loop publishes it into a
double buffer (two preallocated snapshots and an atomic index), so the loop takes no lock.

## Data folder and the workspace

`ams2-platform` implements the Stintmate workspace contract's resolution (`--workspace`,
`STINTMATE_WORKSPACE`, the pointer file `%APPDATA%\Stintmate\settings.json`) and checks it
against the contract's test vectors. The result is the data folder
`<workspace>\connectors\ams2\` (`ams2` is the connector id), resolved once per process. A
workspace error never falls back to anything and nothing is written: command lines print
`workspace error <code>: <message>` and exit with status 3; the control app shows a dialog and
starts nothing. The control app is the only process that starts others and passes its resolved
`--workspace` to each, so parent and child cannot disagree.

## The links

Every service process is a **client** and serves nothing; the control app serves the
connector's own endpoint.

- **The `pitwall` link**: Pitwall's endpoint (a named pipe per user). The process declares its
  methods and channels before it connects, connects, runs its per-connection hooks (the
  catalog, guidance and actions declarations), and reconnects with the protocol's backoff
  (0.25, 0.5, 1, 2 s, then every 2 s with 20 % jitter). Refused with `connector_busy` (another
  connector id is connected) or `unauthorized` (another key): state `refused`, wait 10 s.
  Pitwall absent: `waiting`.
- **The control link**: the recorder's connection to the control app's endpoint, declaring the
  state channel `service.status`. A recorder started by hand without the control app just keeps
  trying.
- **The key** is the workspace's `ipc\key`, only read, never created by a connector; while it
  is missing the links say so and retry, and it is re-read when it changes.
- **Status**: one record per service goes to Pitwall as `connector.status` (the contract's
  fields only) and to the control app as `service.status` (the same plus the pid, the Pitwall
  link state, counters and CPU), each set only when its value changed. The connection is the
  liveness; there is no status file.
- **Declarations** are idempotent: the announcement catalog, the engineer's guidance and the
  actions are declared at every connection with an idempotency key made of the SHA-256 (first
  12 hex digits) of their canonical JSON, so reconnecting changes nothing in Pitwall.

## The outbox

Every write goes through the disk, so laps recorded while Pitwall is closed are delivered when
it opens, and never twice.

1. The writer thread turns a record into the params of its write method and stores one file
   `<20-digit sequence>.msgpack` in `outbox\` holding `{method, v, ik, p, at}`, written to a
   temporary name and renamed. One file per lap and per session message (the VR rule: one disk
   write per lap). A 90 s lap (4500 samples of 72 channels, as raw float32) is about 1.30 MB.
2. The sender thread sends them in sequence order, one at a time, each waiting for its answer
   (the write methods are ordered), with the key stored in the file.
3. `ok`: the file is deleted. `bad_request`, `session_closed`, `unknown_session`, `too_large`
   (or an unreadable file): moved to `outbox\rejected\`, logged and counted; a refused rival
   batch is queued again row by row so one odd row does not lose the batch. Anything else
   (`busy`, `timeout`, connection lost, no answer within 30 s): the file stays and is sent
   again after `retry_ms` (1 s by default) or after the reconnect, with the same key: Pitwall
   stores a write whose answer was lost once.
4. Files an earlier run left are loaded at start and go first; the sequence continues after
   the highest number on disk.
5. `connector.status.outbox` is `{pending, oldest_at, rejected}`. A clean stop stores what is
   queued and delivers what it can within 5 s.

**Sessions by key.** The tracker numbers its sessions locally and each gets a random
`session_key` (16 hex digits) when it opens; every write carries it, never Pitwall's session
id, which the connector may not know yet (a session can be started, driven and ended while
Pitwall is closed). Idempotency keys: `ams2/<session_key>/start`, `.../lap/<seq>`,
`.../setup/<seq>`, `.../pit_exit/<seq>`, `.../participants/<seq>`, `.../end`, and
`ams2/catalog/<hash>`.

## Shared memory reader

`ams2-shm` owns the layout of AMS2's shared memory (the Project CARS 2 format, version 14,
natural alignment, 20 700 bytes). Size and field offsets are asserted at compile time, and a
test checks them against offsets generated from the game's header, so a layout mistake does
not build. `Mapping` opens `$pcars2$` read-only with `OpenFileMappingW`, never creating it, and
takes a consistent snapshot with the sequence-number pattern into a preallocated buffer (retry
on odd or changed values, up to 10 times, then skip the tick). The reader sits behind a
`FrameSource` trait so the core can be driven by a test mapping, a raw capture or a scripted
session. Units, signs and the reasons behind each choice: [FINDINGS.md](FINDINGS.md#shared-memory).

## Recorder

- **Sampling**: the shared memory at 50 Hz (`--hz`, the control app's `recorder_hz`), a
  high-resolution waitable-timer sleep between ticks, nothing written per tick. Per tick: one
  snapshot, the tracker, the announcement detector (a few integer comparisons) and a clock read
  for the status. Below-normal priority.
- **Tracker** (`ams2-core::tracker`): sessions, laps, pit exits, setups. A lap ends when
  `mCurrentLap` increments **or** the lap distance wraps on track (AMS2 does not increment the
  counter on the first crossing after the garage); the formation lap is split where the timing
  starts; a stale lap counter when a race loads restarts the lap instead of opening a session;
  a finished lap waits up to 3 s for `mLastLapTime`, else uses the game lap clock at its last
  sample. Laps are numbered 1..n per session (the game's number is kept as `sim_lap`) and get a
  type (`formation`, `out`, `in`, `flying`). **Stints are Pitwall's**: at each pit exit the
  tracker flushes the pending lap, closes the open one as partial if the car had been on track,
  and queues `pit_exit` and then the setup snapshot, in that order. The quirks behind these
  rules: [FINDINGS.md](FINDINGS.md#game-quirks).
- **Rivals**: one row per participant and lap, the safety car excluded, delivered in batches
  at 2 Hz.
- **Mapping to Pitwall's model**: channels (units and signs of Pitwall's channel catalogue),
  vocabulary (session types, pit modes, surfaces), setup snapshot rules, display names
  (embedded; a readable fallback), and the write messages.
- **Writes**: the tracker's records are queued, encoded once on the writer thread and stored
  in the outbox; the loop never waits. At a session start the recorder sends the catalogue and
  `session_start`; rival cars not yet catalogued go in a catalogue write before their rows.
- **`connector.live_frame`** (declared inline): the last snapshot converted only when asked:
  `phase` (`garage`, `on_track`, `paused`, `menus`), the session type, Pitwall's pit codes,
  units converted (km/h, litres, degrees C, kPa, seconds), "not given" as `null`, the safety
  car flagged. `lap.current_lap` is the game's own counter, as the protocol defines it.
  Optional fields say why a lap is not timed or valid: `session.started`, `car.penalty`,
  `lap.lap_type`, and once the tracker took the game's flag, `lap.invalid_cause`,
  `lap.invalid_cause_certain` and `lap.invalid_at_m`. The cause is decided once, from the lap's
  own samples in the 10 s before the flag: two or more wheels on an off-track surface or a wall
  is `off_track` (or `track_limits` when every wheel off is on a track-limit strip); else all
  four wheels on kerbs for 0.1 s or more is `track_limits`, not certain; else `unknown`. An out
  lap's own flag never sets a cause. AMS2 gives no cut and no penalty as a cause, so none is
  ever sent. Errors: `game_not_running` (process gone, or no new frame for 1 s while playing),
  `internal` (unsupported version).
- **`connector.status`**: `game_running`, `phase`, `session_key`, `stint` (Pitwall's, as last
  answered), `lap`, `live` (`session_type`, `car_id`, `track_id`, `layout_id`,
  `track_length_m`), `outbox`, `error_code` (`unsupported_sim_version`), `message`.

### Special cases, and why

- **Game detection.** The mapping outlives the game (any process holding it keeps it alive), so
  the connector never trusts the mapping alone. "The game runs" means a process `AMS2AVX.exe`
  or `AMS2.exe` exists, checked before mapping and whenever no new frame came for 5 s, at most
  every 5 s. Game gone: the mapping is closed, the session ended, `game_running` false within
  5 to 10 s. Game still running (menus, pause): the mapping is reopened, since a restarted game
  has a new one. The check is cheap by design: a zero-timeout wait on the game's process handle
  while it runs, otherwise a window lookup by the game's class and title, with a full process
  snapshot only when such a window appears or every 60 s as a backstop (the full snapshot cost
  about 6 ms on the development PC).
- **Return to the garage from the menu.** The tracker takes no samples while the game is
  paused or in a menu, so a lap ended by "return to garage" never had a sample in the garage
  and would be stored as a flying lap. In the paused or menu branch an open lap is marked as an
  in lap as soon as the pit mode shows the pits or the garage, and on the first frame after it
  when the game has reset the lap counter. A lap that started in the garage stays an out lap.
- **The session opens in the garage.** The garage screen is a menu with the pit mode "in the
  garage". Opening the session only when the car left meant that captures taken in the garage
  had no session, car or track. The tracker now opens the session from a garage frame (pit mode
  in the garage, paused or in menu, a valid player car, car, track and layout, a session state
  other than invalid) and sends the catalog, `session_start` and the setup snapshot, nothing
  else. Trade-off: a session loaded and left without driving is started and then discarded (the
  write API stores a session only with a lap), which leaves a gap in Pitwall's session ids.
- **In the car in the garage, and laps that never moved.** Pressing Drive leaves the car in the
  garage with pit mode "driving out of the garage" at 0 km/h ([FINDINGS.md](FINDINGS.md#in-the-car-in-the-garage)).
  A small state machine follows the viewed car frame by frame: in the garage = the garage
  screen's pit mode, or "driving out of the garage" since that pit mode began without ever
  exceeding 3 km/h (latched). It drives the recorder's status, the live frame's pit code (4) and
  the telemetry flag. A lap whose top speed stays under 3 km/h is dropped ("the car never
  moved"), so a driver sitting in the car and leaving does not create a junk session. The
  decision stays with the connector: it decides what a lap is.
- **The lap counter in the live frame.** `lap.current_lap` is the game's counter, faithfully,
  and so is the stored lap's `sim_lap`; the connector's own lap number is in the status. AMS2's
  counter does not count line crossings ([FINDINGS.md](FINDINGS.md#game-quirks)), so Pitwall
  numbers laps itself.

## `connector.telemetry`

The neutral 32-byte samples Pitwall's brake-cue scheduler runs on (layout `telemetry`: lap
distance, speed, longitudinal acceleration, track length, lap time, frame, game lap, session
type, game state, flags), declared with `rate_hz: 100`.

- **Only while Pitwall subscribes.** A thread of its own, separate from the loop, maps the
  shared memory itself (its own read-only view, so the two never share a pointer) and samples.
  Nothing runs and nothing is mapped while nobody subscribes.
- **One sample per new game frame**, with `t_ns` taken right after reading the changed
  sequence number (never at send time), then the sequence number is read again (changed or
  odd means torn: read again at once). While no new frame comes, one repeat with `fresh` = 0
  every 250 ms keeps `game_state` current. `o: {rate_hz}` (10 to 200) lowers the cap.
- **Phase-locked waits.** Waking every few milliseconds to poll the sequence number would cost
  more than the samples. The thread sleeps until the next frame is due (the measured frame
  period plus a small lead), then polls every 0.25 ms until the number moves, and every 5 ms
  with no frame for two periods; the expected time is nudged earlier each frame so the lock is
  found again every few frames.
- **Flags** (AMS2 to neutral): `on_track`, `in_pit_lane`, `in_garage` (the garage screen or the
  garage rule), `timed` (`mCurrentTime` >= 0), `flying`, `lap_invalid`, `player_view` (the
  viewed participant is valid and the game is not in a replay), `fresh`.

## Race announcements

The connector never composes user text. It declares its catalog `connector.ams2` (English
texts, typed params, and per message `audience`, `priority` (five levels), `max_wait_s`, `slot`, `kind` and a translator's
note) and sends only `pitwall.announce {code, params, session_key?, data?}`. Pitwall composes
the written and spoken text in the user's language.

- **Detection** (`ams2-core::announce::detector`): pure edge detection over the recorder's
  snapshots, once per new game frame; the per-participant arrays only at 2 Hz. Rules: only
  while playing and the viewed car is the player; nothing in the first 10 s after a session
  load or a session or race state transition; debounce (flags 0.5 s, race states 1 s, pit
  schedule and caution 0.5 s stable); re-arm and hysteresis (a yellow again only after 5 s of
  none or green; "track clear" after 3 s); cooldowns per code (yellow 30 s; the blue flag is per lapping car, see below, 20 s and at
  most 3 each); no flags in the pit lane; blue, penalties and race states in
  races only; the AMS2 caution quirk (none straight to "pits open": `fcy_start`, then
  `fcy_pits_open` 8 s later); once per session for final lap, chequered, disqualified, retired;
  rivals by identity (name and class, never the array index; inactive slots, duplicates of the
  player and the safety car ignored).
- **Whose a flag is** (PRY-090, `detector.rs`: `attribute`, `cause_car`, `rival_ok`). The game
  shows a yellow to every car in a zone around an incident, the car that caused it included, so
  at the yellow edge (after the 0.5 s hold) the detector decides: the player was on track below
  5 m/s within the last 5 s: his own (`flag_yellow_self`, `off_track` when `mCrashState` is not
  NONE, else `stopped`; said once per incident, the incident is then closed silently: no
  escalation call, no `flag_clear`); else the one other car that is slow (< 6 m/s), on track,
  racing, named, not the Safety Car and within 350 m behind to 1500 m ahead on the track, the one
  carrying a yellow itself first, then the nearest: `flag_yellow` / `flag_double_yellow` with
  that car's sector, then `flag_yellow_driver`; else `mCrashState` not NONE: his own; else
  `flag_yellow_ahead` / `flag_double_yellow_ahead` (no sector is made up) and the game's
  reason. A yellow raised at a standstill is deferred with its attribution (always his own)
  and said when the car moves. The black and white right after `mLapInvalidated` rose (1.5 s)
  is `flag_track_limits`; the white slow-car flag names the nearest slower car ahead within
  500 m (`flag_slow_car_driver`) and is silent when the player is the slow one. Cost: two
  comparisons per tick on the player's fields; one pass over at most 64 participants without
  allocation at a flag edge (bench: median unchanged within noise, 1.4-1.6 us).
- **Sending**: params checked against the catalog, at most 10 per 60 s (the contract's limit,
  kept by the connector), without waiting (`call_async`: the loop never blocks); dropped and
  logged while Pitwall is not connected or before the catalog was declared, since an
  announcement is worth something only now.
- **Guidance and actions** for the engineer: constants declared with `pitwall.guidance.declare`
  and `pitwall.actions.declare` (failures logged, never raised). `connector.action` serves
  `test.ping` and `test.announce` on a pool thread that waits at most 2 s for Pitwall to accept
  the announcement; the 50 Hz loop is never involved.

## Capture target

Pitwall's core takes captures of the game screen; the connector only knows which window is the
game. The recorder declares `connector.capture_target {}` (called before each capture, deadline
1 s) and answers `{hwnd, pid, title}` of the AMS2 desktop window (the VR mirror), found with
`EnumWindows` off the loop: by process name first, else by exact title "Automobilista 2" and
class `DefaultAppClassName` (so an Explorer or browser window with that title never matches);
the largest client area wins. Errors: `game_not_running`, `window_minimized`. The connector
sends no image and has no capture dependency.

## The control app and its endpoint

The control app is built in two layers. The heart (`ControlApp`) holds what the tray and the
window show and do behind a small in-process API (commands in, snapshots and events out); it is
free of any UI and unit-tested. The tray (`tray-icon`, `muda`) lives in the same process; the
window is the separate process described above.

- **ServiceManager**: spawns each service suspended, assigns it to one kill-on-close Job
  Object, then resumes it, so a crash or kill of the app kills its services. Clean stop through
  the stop event, forced after 10 s. Adopts instances started by hand (by the PID their
  connection gives). Error notices at most one per service per minute. While the Recorder is
  off it asks whether the game's process runs, so the row can say "Off - Automobilista 2 is
  running: laps are not recorded".
- **Idle cost**: the supervisor sleeps on a condition variable and wakes every 1 s only while a
  service runs, is wanted, stopping or restarting; every 2 s in managed mode (the parent
  check); otherwise every 5 s. `ui.snapshot` and `control.state` are pushed only on change, and
  the tray is told only on change. The autostart registry value is cached for 30 s.
- **The endpoint `connector-ams2`** (a named pipe per user; its registry entry is written to
  `ipc\endpoints\`):
  - control interface for a managing app: `control.show`, `control.hide`, `control.quit`
    (answers `{exiting: true}` first), `control.managed {parent_pid}`,
    `control.service.start|stop {service: "recorder"}`, `control.autostart.set {enabled}`, all
    ordered, plus `control.catalog.get` (the built-in English catalog and its revision).
    Params are checked (`bad_request`); the controller refuses with `busy` when it cannot.
    The caller's PID chain is logged with every start, stop and quit.
  - channels: `control.state` (common fields, the connector's own, and `services[]`),
    `control.activity`, `control.notices` (events for error notices, also tray balloons when
    standalone), `control.services.desired` (only for a switch made in the app's own window or
    tray while managed).
  - the services' `service.status`, subscribed per connection, with the pid replaced by the
    connection's OS PID. A service without a connection has no status.
- **Managed mode**: no tray icon, hidden window, no Exit or autostart switch, notices only on
  the channel; the parent PID (and creation time) checked every 2 s, falling back to
  standalone when it is gone. A `control.service.*` call is not saved as the user's choice (the
  managing app keeps it) and never opens the window.
- **Standalone**: adopts instances started by hand, starts each service whose "start with the
  app" setting is on, and saves a hand switch. Exit and logoff do not change the setting.
- **Single instance**: a named mutex; a second launch calls `control.show` (or
  `control.managed`) on the running one and exits.
- **Connector descriptor**, the per-user machine file
  `%LOCALAPPDATA%\Stintmate\connectors\ams2.json` (schema 1; PRY-088): id, name, sim, sim name,
  version, the absolute path of the control app as `launcher`, `args`,
  `control_protocol: "stintmate-ipc/2"`, `endpoint: "connector-ams2"`, the mutex, the services,
  the `logs` (in `<workspace>\connectors\ams2\`) and two optional keys: `flavour` (`dev` for
  the development build) and `uninstall` (the argv that removes this install). Written
  atomically by the setup, through `--register`, and never by the control app at start; removed
  by `--unregister` only when `launcher` names this install; read for the next setup by
  `--read-registration`. It needs no workspace, so a managing app finds the connector without
  knowing the simulator and before any workspace exists. `STINTMATE_CONNECTORS_DIR` replaces
  the folder in tests.
- **Texts**: every UI text is a key of the built-in English catalog `ams2-connector`; the
  language and packs follow the Stintmate text catalogs contract (a chain such as `es-CL`,
  `es`, `en`, built in; entries validated one by one; plurals from the pack's number rules).
  Not localised: logs, JSON files, IPC messages and ids, the descriptor.

- **A held blue flag follows the train of lapping cars** (BUG-139). While the game's blue flag holds in a
  race (not in the pit lane, not below 1 m/s) the detector looks, at the rival period and not every frame,
  for the nearest car behind on track that is a lap or more ahead in the race. The first call of an
  episode (the flag up until it drops) is `flag_blue` ("Blue flag, let {driver} through."); each further
  car of the train only `flag_blue_next` ("Let {driver} through."), at least 4 s after the previous blue
  call. Cooldown and limit are per car: the same car is not called again within 20 s, at most 3 times a
  session. A blue flag already up when the car leaves the pit lane (or starts moving) is called then, as
  the first call of the episode. With nobody to name only the first call (`flag_blue_unknown`) is made.
- **Spotter** (`ams2-core::announce::spotter`, PRY-098): the one part of the detector that sees every
  frame (the rest reads the participant arrays at 2 Hz). AMS2 has no proximity field, so it works from
  `mWorldPosition`, `mOrientation[1]` and `mSpeeds`: in the player's frame (`theta = -yaw`) a car is
  `x'` to the left and `z'` behind; a new overlap needs `|x'| > 1.8 m` and `|z'| < 4.4 m` ahead / `< 4.8 m`
  behind, a held one stays while `|z'| < 4.9 m` (hysteresis); cars further than 20 m, in the pit lane or
  closing at 12 m/s or more do not count. One state machine of what the driver was told (left 0/1,
  right 0/1): each change sends one call, all in the slot `spotter` so a newer call replaces a queued
  one; a side must stay empty 150 ms before its clear; an overlap back within 1.5 s of its clear is
  called only if it still holds; a side-specific still call (`spotter_still_right|left|middle`) every 3 s since
  the last call, in one of its catalog wordings (`variants`, PRY-100) picked by splitmix64 and never the one
  before it; "All clear" after "In the
  middle". Gates: practice, test, qualifying and race (not time attack, formation lap, private
  sessions), the player on track at 10 m/s or more, a race from 20 s after the green, no full-course
  yellow; a closing gate resets the state silently. The `Announcer` keeps a second `RateWindow` (120 per
  60 s) for the `spotter` level. The tick runs it without allocating; its cost is in
  [BENCHMARKS](../BENCHMARKS.md).

## Names

Ids are AMS2's own strings (car = `mCarName`, class = `mCarClassName`, track =
`mTrackLocation`, layout = `mTrackVariation`); display names come from an embedded table
(classes, cars, tracks, layouts keyed `<track_id>/<layout_id>`), else a readable fallback.

## Performance

The recorder must have minimal impact on the game, which often runs in VR: low priority, a
sleep between samples, one disk write per lap, no allocation on the loop. Measured with
`tools/measure` on a synthetic on-track scenario (the real recorder loop at 50 Hz in its own
process, a test mapping written at 90 frames/s, a fake Pitwall subscribing in alternate 15 s
windows), release build, 3.79 GHz, CPU from `QueryProcessCycleTime`. 2026-10-23, median of 3
runs, on a PC that was busy with other work, so tails are noisy and medians are the figure to
trust. Full tables and method: [`BENCHMARKS.md`](../BENCHMARKS.md).

| Metric | Target | Measured | Met |
|---|---|---|---|
| Recorder CPU at 50 Hz, no subscription | <= 0.2 % | 0.127 % | yes |
| Recorder CPU with the stream at 90 frames/s | <= 0.4 % | 0.490 % (2026-10-25) | no |
| What the stream adds | <= 0.2 % | 0.363 % (2026-10-25; about 0.09 % is the sampling thread, about 0.27 % the IPC send) | no |
| Allocations on the loop per tick, steady state (status ticks included) | 0 | 0 | yes |
| Loop period p99 | <= 20.5 ms | 20.63 ms | within 0.15 ms |
| `t_ns` after the frame (median) | <= 0.5 ms | 0.53 ms | within 0.04 ms |
| Tick median (snapshot, tracker, detector, live publish) | <= 5 us | 9.4-9.8 us | no |
| A 90 s lap encoded and stored | <= 5 ms | 4.7-5.0 ms (1.30 MB) | yes (marginal) |
| Start to first snapshot, in process | <= 0.3 s | about 2 ms | yes |
| Recorder private working set | <= 15 MB | 7.3 MB | yes |
| Tray process idle, window hidden | <= 0.05 % | 0.021 % | yes |

For comparison, the earlier Python recorder measured 0.55-0.59 % without the stream and 1.03 %
with it. The Rust recorder is about four times cheaper without the stream and more than twice
with it. A target missed by little is reported with its numbers, not traded for robustness.

What is still open, and why: the 20 kB snapshot copy (2.2 us) and the tracker (4.1 us) dominate
the tick. The status is now a fixed struct (`status.rs`): the loop fills it in place every 0.5 s,
compares it field by field with the last one handed over and, only when it differs (its uptime
changes each second), copies it into a slot and wakes a publisher thread that builds the JSON;
the allocation test now asserts zero allocations on every steady-state tick, status ticks
included. The stream's cost did not move (0.349 % before, 0.363 % after, medians of 3, within
noise): with the IPC send skipped the stream adds 0.09 % (the sampling thread: wake-ups, the
mapping reads, the sample); the other 0.27 % is `StreamChannel::send` in `stintmate-ipc` (a
`Bytes` copy, the subscriber lock and one `WriteFile` per sample, already written by the
publishing thread with no pump wake-up). Fewer writes need the library to coalesce the queued
samples of a subscription into one write (the wire format and the samples stay the same); that
is a change in `stintmate-ipc`, not here. The lap's encoded bytes equal the earlier
implementation's (checked by a test), so Pitwall sees no change.

## Decisions and their reasons

| Decision | Why |
|---|---|
| Rust, a total replacement of the earlier Python connector | A real-time loop that allocates nothing and cannot be paused by a garbage collector or the interpreter; four times cheaper CPU; one self-contained exe set, no runtime to install; the shared memory layout is checked at compile time |
| Contract unchanged | The pipes, the descriptor and the protocol are the interface; Pitwall, the hub and Voice needed no change |
| Pure `ams2-core` with an injected clock | The rules that decide what a lap is (the expensive part to get wrong) are tested without a game, a pipe or a clock, and compared against recorded and scripted sessions |
| Two long-lived processes plus a window on demand | A UI must not share the 50 Hz loop's process; a toolkit's graphics context is paid only while the window is open |
| eframe/egui with `tray-icon` and `muda` for the UI | Permissive licences, small binary with the OpenGL renderer (about 6 MB, 0.03 % idle in the spike), tray events delivered while the window is hidden; not Slint, whose licence carries attribution terms for every fork; native Win32 stays the fallback |
| Disk outbox, one file per write | Laps survive Pitwall being closed and a crash; one disk write per lap keeps VR smooth; the stored idempotency key makes a retry safe |
| Sessions keyed by the connector, not by Pitwall's id | A session can start and end while Pitwall is closed |
| The stream has its own thread and its own view of the mapping | Sampling at the game's frame rate must not disturb the 50 Hz loop's buffers; it costs nothing when nobody subscribes |
| The live frame is converted only when asked | The loop never pays for it; a double buffer, not a lock, hands over the snapshot |
| The connector sends codes, never text | Pitwall translates and speaks in the user's language; the connector stays free of localisation |
| Game detection by process, not by mapping | The mapping outlives the game |
| Dropped on purpose: the schema v5 importer, a JSON channel wire format, batch launchers | Nothing used them; no migration or compatibility code before a release |
| Apache-2.0, permissive dependencies only (checked by `cargo deny`) | Anyone can fork or learn from it; the trademark notice protects the names |

## Tests

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The tests never touch the game, its folder, Pitwall's data or a user's endpoints: every
endpoint runs on a unique pipe name with a random key and a temporary workspace.

- `ams2-shm`: the layout against generated offsets, property tests of the seqlock against a
  writer thread.
- `ams2-core`: the tracker, the garage rule, rivals, the live frame against Pitwall's fixture and
  JSON Schema, the announcement catalog (canonical JSON and hash), the detector (every code's
  edge, gating, cooldowns, identity, cost), golden comparisons and parity tests against the
  earlier implementation's outputs.
- `ams2-recorder`: a fake Pitwall (an endpoint built with `stintmate-ipc`, applying the write
  methods to an in-memory writer that checks each message against the write API's schema, and
  able to lose an answer, refuse a write or restart) running the scripted session (main menu,
  garage, out lap, flying laps including an invalid one, a pit stop, return to garage, menus,
  game closing) through the real recorder, outbox and link. Also: Pitwall closed during the
  whole session and opened afterwards, the outbox of a stopped recorder, a rejected write and
  its split rival batch, `connector_busy`, a wrong key; an allocation-counting test; the
  `lap_bytes` test; `lap_write` (timing, ignored by default).
- `ams2-connector-ui`: the endpoint over a real pipe, the services, managed mode, the
  descriptor, and the conformance tool `stintmate-ipc-check server` against the endpoint.
- `ams2-connector-window`: the window's lifecycle, and the offscreen renders of its pages
  (`tests/render.rs`).

What still needs the real game: the process names on a given PC, "return to garage" in the menu,
closing AMS2 with another program holding the mapping, a real capture, the stream's cost in VR,
every announcement code in a real race, and the tray's real context menu.
