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

Build the connector (see the root [README](../README.md)) and run, once:

```
target\release\ams2-connector-ui.exe --register
```

`--register` writes the connector descriptor `<workspace>\connectors\ams2\connector.json`, which
tells a managing app (the Stintmate hub) where the connector is and how to start and control
it. The descriptor stores the absolute path of the exe, so register again after moving it. The
control app also refreshes the descriptor at every start, and `--unregister` removes it (see [Command lines](#command-lines)). The three executables must stay in
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
race. Its panes:

- **Services**: the Recorder with its switch, state and detail lines (CPU, how many laps wait
  for Pitwall), the data folder and where it comes from, and **Open data folder**.
- **Options**: start with Windows, start minimized, the recorder's sampling rate, which log the
  Logs pane shows first.
- **Logs**: the tail of `recorder.log` and `ui.log`, with dates in your system's format.

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
ams2-connector-ui.exe --register   [--workspace DIR]
ams2-connector-ui.exe --unregister [--workspace DIR]
ams2-connector-ui.exe --quit
ams2-connector-ui.exe --version
ams2-recorder.exe [--hz 50]            (one instance; Ctrl+C stops it)
ams2-recorder.exe --version
ams2-connector-window.exe --version
```

`--register`, `--unregister`, `--quit` and `--version` start nothing and cannot be combined with
each other or with `--managed` / `--tray`. A bad command line prints the usage (every option)
and exits 2.

| Option | What it does | Exit status |
|---|---|---|
| `--register` | writes `<workspace>\connectors\ams2\connector.json` (this exe's path, the endpoint, the mutex) and exits; left alone when it is already up to date | 0 done, 1 cannot write, 3 workspace error |
| `--unregister` | removes that file if it is this connector's (`"id": "ams2"`); logs, outbox and `ui_config.json` stay | 0 removed or nothing to remove, 1 cannot remove (or not this connector's file), 3 workspace error |
| `--quit` | asks the running control app to exit and waits up to 10 s until nothing of the connector runs | 0 stopped (or nothing ran), 1 still running or the app does not answer, 4 the app is managed |
| `--version` | prints `<exe name> <version>` (for example `ams2-connector-ui 3.0.0`) | 0 |

`--quit` finds the instance by the single-instance mutex `Local\Ams2Connector.UI` and sends
`control.quit` to its endpoint `connector-ams2` with the workspace's key (the same call the
managing app uses; the app stops its recorder first, which can take a few seconds). A recorder
left running without the app is asked to stop through its stop event. A managed app refuses
(error `managed`) because its managing app must stop it: `--quit` then does nothing and exits 4.

The control app writes the descriptor at every start (when it finds a workspace), so a moved or
updated exe fixes it by itself; `--register` is for installers and scripts.

Output of `ams2-connector-ui.exe` and `ams2-connector-window.exe` (no console of their own):
when started from a console, they attach to it and print there; redirected output (a file, a
pipe, a script capturing it) is left as it is. A shell does not wait for these programs: use
`Start-Process -Wait` or `start /wait`, or pipe the output (`... | Out-String`), to get the
text and the exit status in order. `ams2-recorder.exe` is a console program.

All of them take `--workspace <dir>` and `--allow-synced` (accept a workspace inside OneDrive,
refused by default) where a workspace matters. The control app passes its resolved workspace to
every process it starts.

Subcommands of `ams2-recorder.exe`:

- `announce CODE [--param NAME=VALUE ...] [--data JSON]`: send one of the connector's
  announcement codes through Pitwall (a test hook). The code and its parameters are checked
  before sending (exit 2 otherwise). Prints the message id (exit 0) or the refusal (exit 1).
- `codes`: list the catalog.
- `diagnose`: print one consistent snapshot's main fields and the mapping size (exit 1: no
  mapping; 2: unsupported version).
- `capture-raw FILE`: record the game's frames for test fixtures. Only in builds made with
  `--features capture-raw` (development builds); never started by the control app.

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

Beside it in the workspace: `connector.json` (the descriptor), `ipc\key` (the IPC key shared by
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
([ARCHITECTURE.md](ARCHITECTURE.md#race-announcements)). The catalog `connector.ams2` has 39
codes; `ams2-recorder.exe codes` lists them with their audience, priority and parameters.

| Group | Codes |
|---|---|
| Flags | `flag_yellow`, `flag_double_yellow`, `flag_yellow_cause`, `flag_clear`, `flag_blue`, `flag_blue_unknown`, `flag_slow_car`, `final_lap`, `flag_red`, `flag_black_white`, `flag_black_orange` |
| The player's race | `disqualified`, `retired`, `chequered`, `chequered_no_position` |
| Safety car (full course yellow) | `fcy_start`, `fcy_pits_open`, `fcy_pits_lead_lap`, `fcy_last_lap`, `fcy_restart`, `fcy_halt` |
| Penalties | `penalty_drive_through`, `penalty_stop_go`, `penalty_served` |
| Rivals | `rival_disqualified`, `rival_out` |
| Tests | `radio_check`, `engineer_test` |

Each is built from the game's header and from experience with other programs; confirm them in
a real race before relying on them.

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
