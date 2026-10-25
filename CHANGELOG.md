# Changelog

## 3.0.0 (unreleased)

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
