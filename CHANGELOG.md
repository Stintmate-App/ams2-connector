# Changelog

## 3.0.0 (unreleased)

- Installer (`installer/ams2-connector.iss`, `scripts/build-installer.ps1`): per user, no
  administrator rights; an unattended mode for Stintmate (`/WORKSPACE`, `/MANAGED`, `/PROGRESS`:
  JSON progress lines, reasons `connector_running`, `no_workspace`, `register_failed`, `disk`),
  documented in `docs/installer.md`. `cargo deny`, `cargo about` and a `.sha256` are part of the build.
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
  exe. The descriptor is still written at every start.
