# Changelog

## 3.3.1+docs.2 (2026-11-01)

- The repository is prepared for being public: the workflows run on GitHub-hosted Windows runners only (the `runner` input is gone), `stintmate-ipc` is fetched over HTTPS (the deploy key and the SSH rewrite are gone), `SECURITY.md` says how to report a vulnerability, and the README has a "Contributing" section. The code is unchanged.

## 3.3.1+docs.1 (2026-11-01)

- Documentation, doc comments and comments corrected against the code; the code is unchanged. The
  documents point at the connector-kit instead of contracts a reader cannot open, say what the
  fixtures and their generator scripts are (made once with an earlier implementation that is not in
  this repository), and drop figures that cannot be reproduced. Corrected: the refused-connection
  wait ("about 10 s"), the CPU figure, the installer's parameter and hold descriptions, the tests
  list, the kerb-cause rule and the capture scripts' README. No release is cut for this section.

## 3.3.1 (2026-11-01)

- Uses `stintmate-ipc` v0.3.2. The installer is built on GitHub-hosted runners. Behaviour unchanged.

## 3.3.0+docs.1 (2026-11-01)

- Releases are published by the `release` workflow (`.github/workflows/release.yml`, run by hand):
  it checks the tag, runs the CI checks, builds the installer and uploads it with its `.sha256`
  file. README, "Releases". The code is unchanged.

## 3.3.0 (2026-10-31)

- **The installer adds a Start menu shortcut** named like the connector (not on the desktop; the
  uninstaller removes it). It never starts a connector of its own: with Stintmate installed it opens
  Stintmate, which starts and supervises the connector; without Stintmate, a message says the
  connector needs it and offers https://stintmate.app.
- New option `ams2-connector-ui.exe --from-shortcut` does this. Started by hand without it, the
  control app behaves as before. Stintmate is found through the uninstall entry of its installer; an
  entry whose program is gone counts as not installed.
- The message is part of the connector's language catalog and translates like the other texts.

## 3.2.3+docs.1 (2026-10-31)

- The documents link to the public connector kit. The behaviour is unchanged.

## 3.2.3 (2026-10-31)

- Built on stintmate-ipc v0.3.1, which is stricter on malformed messages; the connector already sent valid ones.
- **`ams2-recorder --hz` checks its value.** `0`, a negative number, a word or a missing value is now a usage
  error (a message on stderr, exit code 2) instead of a crash or a silent fallback to 50 Hz.
- Test fixtures and test names no longer carry a personal name or internal task numbers; comments and
  scripts that pointed at documents outside this repository were reworded. No change in behaviour.
- The unused UI spike (`spikes/ui`) was removed.

## 3.2.2+docs.1 (2026-10-31)

- Documentation corrected against the code. The behaviour and the protocol are unchanged.

## 3.2.2 (2026-10-29)

- **A lap waited in the pit box mid race is no longer a formation lap.** `formation` applies only
  before the first timed lap of the race session; an untimed, non-out lap after it (the box has no lap clock) is
  stored as `in`, in the live frame too.

## 3.2.1 (2026-10-29)

- **The blue flag train survives a short drop of the flag and names each car once.** A train ends only
  after the flag has been off for 20 s; a flag back within it continues with `flag_blue_next`; a car already
  named in the train is not named again.

## 3.2.0 (2026-10-28)

- **Message variants.** A catalog message may declare `variants` (1 to 8 other wordings of the same
  message); `pitwall.announce` carries `variant` (omitted for 0, the `text`). `spotter_still_there` is replaced by
  `spotter_still_right`, `spotter_still_left` and `spotter_still_middle`, each with variants chosen at random
  and never the one before it. `test.announce` and `ams2-recorder announce` accept `variant`.
- **A held blue flag follows the train of lapping cars.** The first car is called `flag_blue`, each
  new car of the train `flag_blue_next` (new code, "Let {driver} through.") at least 4 s later; cooldown (20 s)
  and limit (3) are per car; a blue flag already up when the car leaves the pit lane is called then.

## 3.1.0 (2026-10-27)

- **Spotter calls.** "Car left", "Car right", "In the middle", "Still there", "Clear left / right"
  and "All clear" (seven codes, group `spotter`, level `spotter`, one slot) from the other cars' world
  positions in every frame (`announce/spotter.rs`, CrewChief's geometry). Only in practice, qualifying and
  race (from 20 s after the green), on track at 10 m/s or more, not under a full-course yellow, not in a
  private session. A second rate window of 120 per 60 s serves the `spotter` level beside the 10 per 60 s.
  `spotter` is a valid catalog group. `ams2-measure --cars N` puts traffic around the player.

## 3.0.3 (2026-10-27)

- **Radio priority levels, maximum waits and slots.** Every catalog message declares one of five
  levels (`critical` for yellows, red, FCY start/restart/halt and black-orange; `high` for blue and slow-car
  flags, own yellow, penalties; `low` for rival messages), a `max_wait_s` where the message goes stale (none for
  red, black-orange, FCY start/halt, penalties, disqualified, retired) and a `slot` (`local_flag`, `blue_flag`)
  so a newer message replaces a queued one. `ttl_s` is gone from the catalog and refused by its validation.

## 3.0.2 (2026-10-27)

- `ams2-recorder.exe --shared-memory-name NAME` (development and tests): opens a named test mapping
  instead of the game's and counts the game as running, so `ams2-replay` can drive the real
  recorder. The game's own mapping name is refused. Default behaviour unchanged.

## 3.0.1 (2026-10-26)

- stintmate-ipc v0.3.0 (the core and the conformance tool; compiled in as before). The core's
  public API only gains items and the protocol is unchanged; a reconnect policy no longer holds
  a client's close back.

## 3.0.0 (2026-10-25)

- **Flag calls know whose flag it is.** The game shows a yellow to the car that caused
  it too, so the radio no longer calls the player's own stop a yellow for "a car blocking the
  track". New catalog codes: `flag_yellow_self` (the player's own yellow: "You're off..." /
  "You're stopped on track..."; the incident is closed silently, no `flag_clear` after it),
  `flag_yellow_driver` (another car is the cause: `flag_yellow` / `flag_double_yellow` now carry
  that car's sector, then "{driver} is stopped on track."), `flag_yellow_ahead` /
  `flag_double_yellow_ahead` (the cause is unknown: no made-up sector), `flag_track_limits` (the
  black and white right after the game invalidated the lap) and `flag_slow_car_driver`. The
  deferred standstill yellow now carries only the self note. The announcement notes
  of the replaced meanings are in the catalog; `catalog.json` has 34 codes.

- **`capture-raw` is usable on a real session (dev builds only).** It waits for the game's
  mapping, ends cleanly on a stop file, Ctrl+C / Ctrl+Break, the game exiting or `--seconds`
  (default 3 h), writes a `FILE.json` summary (refreshed every 10 s), uses a 4 MB write buffer
  at below-normal priority, and the reader keeps the whole records of a truncated file.
  `scripts/capture/` has the start, stop and status scripts for the engineer.

- **A recorder restart keeps the game session.** A recorder stopped while the game
  runs no longer sends `session_end`: it leaves `session-resume.json` (key, write sequences,
  game pid, session clock) in the data folder, and the next recorder continues the same session
  key when the pid, car, track, layout and session type match and the game's own clock
  continued: `mEventTimeRemaining` not higher than at the stop (2 s of tolerance; a session that
  began again has the total), the player's `mLapsCompleted` not lower, `mLapsInEvent` and
  `mSessionDuration` equal (`ams2_shm::clock`). No time window. While no recorder runs, the
  control app reads those fields once a second (only while the file exists) and deletes the file
  when the game goes to its menus or exits, the session type, car or track changes, or the clock
  contradicts the stored one.

- **Updating without closing Stintmate.** Two new subcommands of
  `ams2-connector-ui`, `--stintmate-hold [--purpose update|uninstall] [--wait N]` and
  `--stintmate-release [--no-start]`, call the Stintmate hub (`hub.connector.hold` and
  `hub.connector.release`, id `ams2`) to stop only the connector while a setup replaces its
  files and to start it again afterwards. Exit statuses 5 (no Stintmate to ask), 6 (it did not
  answer) and 7 (a session is being recorded). The setup and the uninstaller use them (unless
  `/MANAGED`): "close Stintmate" stays only as the fallback when the hub cannot be asked;
  documented in `docs/installer.md` and `docs/usage.md`.

- **The connector descriptor moves to the per-user machine
  folder** `%LOCALAPPDATA%\Stintmate\connectors\ams2.json` (`STINTMATE_CONNECTORS_DIR`
  replaces the folder in tests). `<workspace>\connectors\ams2\connector.json` is gone; the logs
  stay in `<workspace>\connectors\ams2\`. Schema 1 with two new optional keys, `flavour` and
  `uninstall`. `--register [--flavour NAME] [--uninstall EXE]` needs no workspace (exit 3 is
  gone); `--unregister` removes the file only when its `launcher` names this install; new
  `--read-registration <out-file>`; `--check` is removed; the control app no longer rewrites the
  descriptor at start. The installer no longer needs a workspace (`/WORKSPACE` and the
  `no_workspace` reason are removed), installs one flavour at a time (it removes the other
  through the `uninstall` of the file: progress stage `replace`, reason `replace_failed`) and
  registers with `--uninstall` (and `--flavour dev` for the dev build).

- Installer (`installer/ams2-connector.iss`, `scripts/build-installer.ps1`): per user, no
  administrator rights; an unattended mode for Stintmate (`/MANAGED`, `/PROGRESS`:
  JSON progress lines, reasons `connector_running`, `replace_failed`, `register_failed`, `disk`),
  documented in `docs/installer.md`. Everything foreseeable is checked before anything is
  copied; no rollback, a late `--register` failure ends
  with status 9 and `"installed":true`; unattended runs never close Stintmate. `cargo deny`, `cargo about` and a `.sha256` are part of the build.
- `AMS2_CONNECTOR_DEV_INSTANCE=<tag>` (development only): a second connector with its own
  mutexes and pipes, for tests next to an installed one.

- The connector rewritten in Rust: the shared memory reader, the recorder and the control app.
  Replaces the Python connector (2.0.0) entirely; its pipes, descriptor and contract with
  Pitwall and the Stintmate hub do not change.
- Release build: all three executables carry the icon and the Windows version information
  (product, description, version, copyright, original file name) and answer `--version`;
  release binaries are stripped of symbols (unwinding kept) and never contain the `capture-raw`
  development feature.
- `ams2-connector-ui.exe`: `--unregister` removes the connector's descriptor and keeps its data;
  `--quit` asks a running unmanaged instance to exit (exit 4 when it is managed); a bad command
  line prints the usage with every option; the options print into the console that started the
  exe.
