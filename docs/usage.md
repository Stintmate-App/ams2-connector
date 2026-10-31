# AMS2 connector: usage

How to run the connector. How it works inside: [ARCHITECTURE.md](ARCHITECTURE.md).

## What you need

- Windows 11 (or 10) and Automobilista 2 with Options > System > Shared Memory = "Project
  CARS 2" (once).
- A Stintmate workspace (the folder Stintmate keeps its data in; its onboarding creates it).
  Without one the connector does not start and says so ([the data folder](#the-data-folder)).
- Pitwall running for laps to be stored. Without it the connector keeps recording: laps wait
  in its outbox and are delivered when Pitwall runs; live tools and announcements need Pitwall.

## Install and register

Build the connector (`cargo build --release`, see the root [README](../README.md)) and run, once,
from the repository folder:

```
target\release\ams2-connector-ui.exe --register
```

`--register` writes the connector descriptor, the per-user machine file
`%LOCALAPPDATA%\Stintmate\connectors\ams2.json`, which tells a managing app (the Stintmate hub)
where the connector is and how to start and control it. It needs no workspace. The descriptor
stores the absolute path of the exe, so register again after moving it: the control app does
**not** rewrite it at start. `--unregister` removes it, only when it names this exe (see
[Command lines](#command-lines)). The three executables must stay in
the same folder: `ams2-connector-ui.exe` (the tray and control app), `ams2-recorder.exe` and
`ams2-connector-window.exe`.

## Before a session

1. Start Pitwall (or let the Stintmate hub do it).
2. Start the control app, `ams2-connector-ui.exe` (or let the hub start it, or start it with
   Windows, below). A tray icon appears. The Recorder starts with the app by default.
3. Start the game. The recorder waits for it ("Waiting for Automobilista 2") and connects to
   Pitwall by itself, reconnecting every few seconds if Pitwall is not there.
4. Check: the tray tooltip and the Services pane say "Connected to Pitwall", and Pitwall's
   status shows the AMS2 connector with its functions.

## The tray and the window

The control app is a small tray process. It uses almost no CPU and has no window or graphics
context while idle.

Tray menu: **Open** (the window), **Recorder** (start or stop; the label follows the state),
**Logs**, **Open data folder**, **Exit**. The icon's colour follows the state; amber means
something wants attention, for example "Off - Automobilista 2 is running: laps are not
recorded".

**Open** starts the window, a separate short-lived process (`ams2-connector-window.exe`) that
is gone when you close it. Open it between sessions rather than leaving it on screen during a
race. It opens at 680x460 (resizable) in a dark theme, with a side menu (Services, Options,
Logs) and the version at its bottom. Its pages:

- **Services**: the Recorder with its status pill (amber waiting, green recording, blue
  connected or in the garage, grey off, red failed), its switch with its state in words, tiles
  for the Pitwall link, the CPU and the sampling rate, a sentence about the game and any laps
  waiting for Pitwall; below, the data folder (shortened to start at your profile) and **Open**.
  When Stintmate manages the connector a line says so.
- **Options**: start with Windows (when not managed), start the Recorder with the app (follows the
  Services switch), start minimized, and the recorder's sampling rate (advanced).
- **Logs**: **Recorder** | **App** (`recorder.log`, `ui.log`), **Only warnings and errors**,
  **Copy last 50** and **Open log file**. Each row shows time, level and message; hover a row
  for the full line. Warnings are amber and errors red.

Closing the window leaves the tray running. **Exit** (tray) stops the services cleanly (forced
after 10 s) and quits. If the app crashes or is killed its services are killed with it.

### Status labels

| Label | Meaning | What to do |
|---|---|---|
| Waiting for Automobilista 2 | The game's process is not running. | Start the game. |
| In the menus | Game running, not driving. | Nothing. |
| In the garage - session N - Connected to Pitwall | The car is in the garage with the session open. | Nothing. |
| Recording - session N - stint S - lap L - Connected to Pitwall | Recording through Pitwall. | Nothing. |
| Off - Automobilista 2 is running: laps are not recorded (amber) | The Recorder is off and the game runs. | Switch the Recorder on. |
| Pitwall not running (amber) | Pitwall's endpoint is absent. | Start Pitwall; the connector reconnects. |
| Refused by Pitwall - see logs (amber) | Another connector is connected (`connector_busy`), or the IPC key differs (`unauthorized`); retried every 10 s. | Close the other connector; check the workspace. |
| Recording - Pitwall not running: the laps wait for it | On track without Pitwall; laps are kept ("N laps waiting for Pitwall"). | Start Pitwall when convenient. |
| Starting... | The service runs but has not connected to the app yet. | Nothing. |

## Options and start with Windows

Options are stored in `ui_config.json` in the data folder:

| Key | Default | Meaning |
|---|---|---|
| `start_minimized` | true | start hidden in the tray |
| `recorder_hz` | 50 | sampling rate, 1 to 200 |
| `logs_default` | `recorder` | the log shown first (`recorder` or `ui`) |
| `start_with_app` | the Recorder on | which services start with the app when it runs alone |

**Start with Windows** writes the per-user Run value `AMS2 connector` =
`"<path>\ams2-connector-ui.exe" --tray` (the registry value is the switch's source of truth;
switch it off and on again after moving the exe). It is off by default. The services come back
as they were left. A managing app keeps its own choice and the setting starts nothing then.

## Managed by another app

`ams2-connector-ui.exe --managed --parent-pid <pid>` runs the app with no tray icon, the window
hidden and no Exit or start-with-Windows switch; the managing app shows it, starts and stops
services and quits it through the control endpoint `connector-ams2`. While managed, quit is
accepted only from the managing process. If the managing app goes away the tray icon comes
back and the services keep running. The Stintmate hub does this; the interface is the
application contract's control interface (see [ARCHITECTURE.md](ARCHITECTURE.md#the-control-app-and-its-endpoint)).

## Command lines

```
ams2-connector-ui.exe [--tray] [--managed --parent-pid N] [--workspace DIR] [--allow-synced]
ams2-connector-ui.exe --register [--flavour NAME] [--uninstall EXE]
ams2-connector-ui.exe --unregister
ams2-connector-ui.exe --read-registration OUT-FILE
ams2-connector-ui.exe --quit [--workspace DIR] [--allow-synced]
ams2-connector-ui.exe --stintmate-hold [--purpose update|uninstall] [--wait SECONDS]
ams2-connector-ui.exe --stintmate-release [--no-start]
ams2-connector-ui.exe --from-shortcut
ams2-connector-ui.exe --version
ams2-recorder.exe [--hz 50] [--shared-memory-name NAME] [--pitwall-pipe PATH] [--control-pipe PATH]   (one instance; Ctrl+C stops it)
ams2-recorder.exe --version
ams2-connector-window.exe --version
```

`--register`, `--unregister`, `--read-registration`, `--quit`, `--stintmate-hold`,
`--stintmate-release`, `--from-shortcut` and `--version` start nothing and cannot be combined with each other or with `--managed` / `--tray`. `--register`, `--unregister`,
`--read-registration` and `--from-shortcut` take no `--workspace`. A bad command line prints the usage (every
option) and exits 2. `STINTMATE_CONNECTORS_DIR` replaces the connectors folder (tests and
development: set it before running `--register` from a checkout, or you take over the real
registration).

| Option | What it does | Exit status |
|---|---|---|
| `--register` | writes `%LOCALAPPDATA%\Stintmate\connectors\ams2.json` (this exe's path, the endpoint, the mutex; no workspace needed) atomically and exits; left alone when it is already up to date. `--flavour NAME` adds `"flavour"` (for example `dev`); `--uninstall EXE` adds `"uninstall"`, the command line that removes this install (`EXE /VERYSILENT /SUPPRESSMSGBOXES /NORESTART`) | 0 done, 1 cannot write |
| `--unregister` | removes that file only when it is this connector's (`"id": "ams2"`) and its `launcher` names this exe (absolute, case-insensitive, `/` read as `\`); logs, outbox and `ui_config.json` stay. A file of another install or id is left as it is | 0 removed, nothing to remove, or left as it is (one line says which); 1 cannot remove |
| `--read-registration OUT-FILE` | for a setup: writes the registration to OUT-FILE as UTF-8 **with a BOM**, `\r\n` line ends, `key=value` lines `launcher`, `dir` (the launcher's folder), `uninstall` (the command), `uninstall_args` (the rest, quoted when they have a space) and `flavour` | 0 written, 2 no valid registration of this connector (nothing written), 1 cannot write OUT-FILE |
| `--quit` | asks the running control app to exit and waits up to 10 s until nothing of the connector runs | 0 stopped (or nothing ran), 1 still running or the app does not answer, 4 the app is managed |
| `--stintmate-hold` | for a setup: asks Stintmate (hub method `hub.connector.hold`, id `ams2`, 5 minutes) to stop the connector and not start it until released; calls again every 0.5 s while it answers `stopping`, then waits until the connector's mutexes are free; `--wait` (default 25 s) bounds both; `--purpose` is `update` (default) or `uninstall`. Takes no `--workspace`: the user's pointer and key | 0 held and stopped, 1 held but something still runs, 2 bad arguments, 5 no Stintmate to ask (no pointer, workspace or key, no hub endpoint and no hub mutex, or the hub does not know the connector), 6 Stintmate runs but did not answer (3 s for the endpoint, 5 s per call, busy, an older hub), 7 a driving session is being recorded |
| `--stintmate-release` | for a setup: `hub.connector.release`; Stintmate starts the connector again unless `--no-start` | 0 released (or nothing was held), 2 bad arguments, 5 no Stintmate, 6 not answering |
| `--from-shortcut` | what the installer's Start menu shortcut runs; it never starts a connector. Stintmate installed: opens Stintmate and exits (also when a connector already runs). Not installed, or its launcher is gone: a Yes/No message says the connector needs Stintmate and offers https://stintmate.app (Yes opens it in the default browser, No exits). The message is in the language of the user's workspace when there is one, English otherwise | 0 done, 1 Stintmate could not be started |
| `--version` | prints `<exe name> <version>` (for example `ams2-connector-ui 3.3.0`) | 0 |

`--from-shortcut` finds Stintmate through the uninstall entry its installer writes
(`HKCU`, then `HKLM`, `Software\Microsoft\Windows\CurrentVersion\Uninstall\{B5E7C0D2-4A1F-4E8B-9C36-5D2F7A91E4C8}_is1`):
its `InstallLocation` plus `hub\.venv\Scripts\stintmate.exe` is the program it opens. An entry whose
program is missing counts as not installed. Started by hand without this option, the control app
starts the connector as described above.

`--quit` finds the instance by the single-instance mutex `Local\Ams2Connector.UI` and sends
`control.quit` to its endpoint `connector-ams2` with the workspace's key (the same call the
managing app uses; the app stops its recorder first, which can take a few seconds). A recorder
left running without the app is asked to stop through its stop event. A managed app refuses
(error `managed`) because its managing app must stop it: `--quit` then does nothing and exits 4.

The control app does not write the descriptor: only the setup and `--register` do (so a stale
build started by hand cannot take the registration back). Register again after moving or
updating the exe by hand.

Output of `ams2-connector-ui.exe` and `ams2-connector-window.exe` (no console of their own):
when started from a console, they attach to it and print there; redirected output (a file, a
pipe, a script capturing it) is left as it is. A shell does not wait for these programs: use
`Start-Process -Wait` or `start /wait`, or pipe the output (`... | Out-String`), to get the
text and the exit status in order. `ams2-recorder.exe` is a console program.

All of them take `--workspace <dir>` and `--allow-synced` (accept a workspace inside OneDrive,
refused by default) where a workspace matters. The control app passes its resolved workspace to
every process it starts.

`--hz` takes a rate in Hz. The control app only passes values from 1 to 200 (its `recorder_hz` is checked against that range); the recorder itself accepts any positive, finite number; `0`, a negative value, a word or a missing value is a usage error (message on stderr, exit code 2). `--pitwall-pipe` and `--control-pipe` replace the two pipe paths the recorder dials (tests only; the control app never passes them).

`ams2-recorder.exe --shared-memory-name NAME` (development and tests only; the control app never passes it) opens the named test mapping instead of the game's `$pcars2$` and counts the game as running, so a replayed session (`ams2-replay`, which writes into such a mapping) drives the real recorder end to end. The game's own name (or any name containing `pcars2`) is refused with exit 2. Without the option nothing changes: the game's mapping, opened only while its process exists. Start the replay first: the mapping must exist, with a first frame, when the recorder opens it.

Subcommands of `ams2-recorder.exe`:

- `announce CODE [--param NAME=VALUE ...] [--variant N] [--data JSON]`: send one of the connector's
  announcement codes through Pitwall (a test hook). The code and its parameters are checked
  before sending (exit 2 otherwise). Prints the message id (exit 0) or the refusal (exit 1).
- `codes`: list the catalog.
- `diagnose`: print one consistent snapshot's main fields and the mapping size (exit 1: no
  mapping; 2: unsupported version).
- `capture-raw FILE [--seconds N] [--stop-file PATH] [--shm-name NAME]`: record every changed frame of the game for the offline comparison and test fixtures; waits for the game, ends on the stop file, Ctrl+C, the game exiting or `--seconds` (default 3 h), and writes `FILE.json`, a summary. Only in builds made with
  `--features capture-raw` (development builds); never started by the control app. Use `scripts/capture/` (its README).

## The data folder

The connector keeps its files in `<workspace>\connectors\ams2\`, resolved once when each process
starts: `--workspace <dir>`, else the `STINTMATE_WORKSPACE` environment variable, else the
folder the Stintmate onboarding saved in `%APPDATA%\Stintmate\settings.json`. There is no
other location and nothing next to the program. If no workspace can be used (missing, not a
workspace, inside OneDrive, a corrupt pointer file) nothing is written: command lines print
`workspace error <code>: <what to do>` and exit with status 3, and the control app shows the
error in a dialog and starts no service.

| File | Written by | What |
|---|---|---|
| `recorder.log`, `ui.log` | each process | logs: every lap stored and delivered, every refusal |
| `outbox\<20 digits>.msgpack` | recorder | writes waiting for Pitwall, one file each (a 90 s lap is about 1.3 MB); deleted once Pitwall stored them; leftovers go first at the next start |
| `outbox\rejected\` | recorder | writes Pitwall refused (`bad_request`, a closed or unknown session): kept for a developer to look at; the control app counts them |
| `ui_config.json` | control app | the options above |

Beside it in the workspace: `ipc\key` (the IPC key shared by
every Stintmate program; the connector only reads it) and `ipc\endpoints\connector-ams2.json`
(the endpoint registry entry, for diagnostics). Pitwall's files are Pitwall's; the connector
never touches them.

## Language

The window and tray show their texts in the Stintmate language (`language` in
`<workspace>\hub\settings.json`), read when the app starts. English is built in; another
language comes from a pack, `<workspace>\locales\<tag>\ams2-connector.json`. A text the pack
lacks or has wrong stays in English and `ui.log` names it. What the connector announces is not
in this catalog: it sends codes, and Pitwall composes them in the user's language.

## Race announcements

While the Recorder runs with Pitwall connected, it watches the race (flags, the safety car,
penalties, the player's and the rivals' race states) and sends **codes** to Pitwall, which
writes and speaks them in the user's language. Rules: only while driving, a quiet 10 s after a
session load or a state change, debounce, cooldowns, once per session for the end-of-race
events, rivals by name; at most 10 a minute; nothing is kept while Pitwall is away
([ARCHITECTURE.md](ARCHITECTURE.md#race-announcements)). The catalog `connector.ams2` has 44
codes; `ams2-recorder.exe codes` lists them with their audience, priority and parameters.

| Group | Codes |
|---|---|
| Flags | `flag_yellow`, `flag_double_yellow`, `flag_yellow_ahead`, `flag_double_yellow_ahead`, `flag_yellow_driver`, `flag_yellow_self`, `flag_yellow_cause`, `flag_clear`, `flag_blue`, `flag_blue_next`, `flag_blue_unknown`, `flag_slow_car`, `flag_slow_car_driver`, `final_lap`, `flag_red`, `flag_black_white`, `flag_track_limits`, `flag_black_orange` |
| The player's race | `disqualified`, `retired`, `chequered`, `chequered_no_position` |
| Safety car (full course yellow) | `fcy_start`, `fcy_pits_open`, `fcy_pits_lead_lap`, `fcy_last_lap`, `fcy_restart`, `fcy_halt` |
| Penalties | `penalty_drive_through`, `penalty_stop_go`, `penalty_served` |
| Rivals | `rival_disqualified`, `rival_out` |
| Spotter | `spotter_car_left`, `spotter_car_right`, `spotter_in_the_middle`, `spotter_still_right`, `spotter_still_left`, `spotter_still_middle`, `spotter_clear_left`, `spotter_clear_right`, `spotter_all_clear` |
| Tests | `radio_check`, `engineer_test` |

The spotter calls ("Car left", "Clear right"...) are not edge events of the race but proximity: the
connector watches the other cars' positions in every frame; a car coming alongside is called as soon
as the overlap is seen, and a side is called clear once it has stayed empty for 0.15 s. They work in practice, qualifying and race (a race from 20 s
after the green), on track at 10 m/s or more, not under a full-course yellow; the rate limit for them
is 120 a minute ([ARCHITECTURE.md](ARCHITECTURE.md#race-announcements)).

The detection rules were written from the game's header and checked on recorded sessions, not on
every situation of a real race: the flag and race-state calls are the least tested, so confirm them
in a race before relying on them.

## Troubleshooting

Pitwall's live tools reach the connector through Pitwall's core. When there is no answer the
tool names the reason in `error_code`:

| `error_code` | Meaning | Fix |
|---|---|---|
| `core_not_running` | Pitwall's core is not running. | Start Pitwall. |
| `connector_disconnected` | The core runs, but no connector process is connected. | Start the control app and switch the Recorder on. |
| `not_implemented_by_connector` | The connector is connected but no connected process offers that method; here it means the Recorder is off. | Switch the Recorder on. |
| `not_on_track` | The game is in the menus (or there is no player car). | Go on track. |
| `game_not_running` | AMS2 is not running, or its data froze (no new frame for 1 s while driving). | Start the game, or wait for it to load. |
| `internal` | The connector could not do it: e.g. the AMS2 window is minimized, or the shared memory version is unsupported. | Restore the AMS2 window; read the message. |
| `busy` | Another capture is running. | Wait. |

Other things to check:

- **Nothing is recorded and the label says "unsupported"**: the shared memory version is not 14
  (`connector.status.error_code` is `unsupported_sim_version`). Run `ams2-recorder.exe
  diagnose`.
- **Laps are not arriving**: look at "N laps waiting for Pitwall" in the Services pane and at
  `recorder.log`; files in `outbox\rejected\` mean Pitwall refused them.
- **"Refused by Pitwall"**: two connectors cannot be connected at once, or the key differs;
  close the other connector or check that both use the same workspace.
- **Anti-virus**: the executables are unsigned; if one is quarantined, restore it and add an
  exception for the folder.

## Reporting bugs

Include the time, Pitwall's session id and lap id, the tool output and the relevant line of
`recorder.log` or `ui.log` from the data folder.

## Installing, and the development instance

The connector is installed by its own setup, per user and without administrator rights; the
unattended mode Stintmate uses, its parameters, progress lines and exit codes are in
[installer.md](installer.md).

**Development only:** the environment variable `AMS2_CONNECTOR_DEV_INSTANCE=<tag>` (letters
and digits; anything else becomes `-`, 24 characters at most) makes a second connector that
shares nothing with the installed one: the single-instance mutexes become
`Local\Ams2Connector.Dev-<tag>.*` and every pipe the three programs build uses the pipe user
`<user>-<tag>` (their own `connector-ams2` endpoint and the `pitwall` pipe they dial), so
pairing it with a Pitwall started with the same pipe naming gives a separate set of endpoints.
It does not move the workspace: use `--workspace <dir>` or `STINTMATE_WORKSPACE` for that. It
is read in one place (`ams2_platform::dev`), is not documented in the program's usage and must
never be set in production.
