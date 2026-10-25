# Changelog

## 3.0.0 (unreleased)

- **A recorder restart keeps the game session (BUG-128).** A recorder stopped while the game
  runs no longer sends `session_end`: it leaves `session-resume.json` (key, write sequences,
  game pid, session clock) in the data folder, and the next recorder continues the same session
  key when the pid, car, track, layout and session type match and the game's own clock
  continued: `mEventTimeRemaining` not higher than at the stop (2 s of tolerance; a session that
  began again has the total), the player's `mLapsCompleted` not lower, `mLapsInEvent` and
  `mSessionDuration` equal (`ams2_shm::clock`). No time window. While no recorder runs, the
  control app reads those fields once a second (only while the file exists) and deletes the file
  when the game goes to its menus or exits, the session type, car or track changes, or the clock
  contradicts the stored one.

- **Updating without closing Stintmate (PRY-089).** Two new subcommands of
  `ams2-connector-ui`, `--stintmate-hold [--purpose update|uninstall] [--wait N]` and
  `--stintmate-release [--no-start]`, call the Stintmate hub (`hub.connector.hold` and
  `hub.connector.release`, id `ams2`) to stop only the connector while a setup replaces its
  files and to start it again afterwards. Exit statuses 5 (no Stintmate to ask), 6 (it did not
  answer) and 7 (a session is being recorded). The setup and the uninstaller use them (unless
  `/MANAGED`): "close Stintmate" stays only as the fallback when the hub cannot be asked;
  documented in `docs/installer.md` and `docs/usage.md`.

- **Breaking (PRY-088, nothing released): the connector descriptor moves to the per-user machine
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
