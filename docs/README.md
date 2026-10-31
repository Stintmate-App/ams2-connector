# AMS2 connector: documentation

The Automobilista 2 connector for [Stintmate](https://stintmate.app). It reads the game's shared
memory and records sessions and laps through Pitwall, Stintmate's telemetry database, over
[Stintmate IPC](https://github.com/Stintmate-App/stintmate-ipc). It is written in Rust, has no
runtime to install, and is also the reference connector for authors who bring another simulator
to Stintmate.

| Document | What it holds |
|---|---|
| [usage.md](usage.md) | Install and register, the tray and the window, options, start with Windows, managed mode, command lines, the data folder, race announcements, troubleshooting. |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Processes, threads, crates, data flow, the IPC links and declarations, the outbox, special cases of the game, performance with measured numbers, decisions and their reasons, tests. |
| [installer.md](installer.md) | The installer: unattended parameters, progress lines, reasons and exit codes, what Stintmate does around it. |
| [FINDINGS.md](FINDINGS.md) | What is known about AMS2's data, with its evidence: shared memory layout, units and signs, game quirks, game files, calibrations and validation. Useful to anyone reading a simulator's data. |
| [writing-a-connector.md](writing-a-connector.md) | A short guide for authors of other connectors, using this one as the map. |
| [../BENCHMARKS.md](../BENCHMARKS.md) | What the connector costs while it runs, with the method. |

The protocols: the connector protocol (`connector-protocol.md`, in Pitwall's documentation in the Stintmate repository)
says what a connector declares and writes; Stintmate IPC v2 is the transport
([specification, vectors and conformance tool](https://github.com/Stintmate-App/stintmate-ipc)).

Everything about the game is read-only: the connector never writes to the game folder or to any
other program's data.
