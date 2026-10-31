# AMS2 connector

The Automobilista 2 connector for [Stintmate](https://stintmate.app): it reads the game's
shared memory and records sessions and laps through Pitwall, Stintmate's telemetry database,
over [Stintmate IPC](https://github.com/Stintmate-App/stintmate-ipc). It is also the reference
connector for authors who want to bring another simulator to Stintmate.

**Status:** released (version 3.3.0). It replaces the earlier Python connector (2.0.0) entirely. It is installed by its own setup, per user, and Stintmate starts and
supervises it ([installer](docs/installer.md)). Its Start menu shortcut opens Stintmate, or tells you where to get it; it never starts a connector alone.

| Path | What |
|---|---|
| `crates/ams2-shm/` | the game's shared memory (version 14): layout, mapping, consistent snapshots |
| `crates/ams2-core/` | the logic without Windows or IPC: sessions, laps, announcements, live frame |
| `crates/ams2-platform/` | Windows helpers: workspace, logging, processes, timers |
| `crates/ams2-i18n/` | the connector's texts and the catalogs contract loader |
| `bins/ams2-recorder/` | the recorder process (50 Hz, no window, below-normal priority) |
| `bins/ams2-connector-ui/` | the control app (tray, supervision of the recorder, the endpoint a managing app uses) |
| `bins/ams2-connector-window/` | the window (Services, Options, Logs), started by the control app on demand |
| `tools/` | development tools, not shipped: `replay` (plays a recorded or scripted session into a test mapping), `measure` (what the recorder costs), `golden` (compares exports of a capture) |

Documentation: [`docs/`](docs/README.md): [usage](docs/usage.md), [installer](docs/installer.md), [architecture](docs/ARCHITECTURE.md),
[what we learned about AMS2](docs/FINDINGS.md) and [writing a connector](docs/writing-a-connector.md).

## Building

Windows only. Requirements: Rust through [rustup](https://rustup.rs) (`rust-toolchain.toml` pins
the toolchain, 1.99, with `rustfmt`, `clippy` and the `x86_64-pc-windows-msvc` target, and rustup
installs it on the first `cargo` call), the Visual Studio Build Tools C++ workload (MSVC linker),
and git: the Stintmate IPC library is fetched by cargo from its repository
(`github.com/Stintmate-App/stintmate-ipc`, tag in `Cargo.toml`). No other tool is needed.

```
cargo build --release
```

builds every program into `target\release\`: `ams2-connector-ui.exe`, `ams2-recorder.exe` and
`ams2-connector-window.exe` (keep these three in one folder), plus the development tools
`ams2-replay.exe`, `ams2-measure.exe` and `ams2-golden.exe`. A first build takes a few minutes.
To run the connector from the build, register it once and start the control app (see
[usage](docs/usage.md#install-and-register)).

Checks:

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Licence and name

Apache License 2.0 ([`LICENSE`](LICENSE), [`NOTICE`](NOTICE)). Every dependency compiled into
the connector is permissively licensed (checked by `cargo deny`, [`deny.toml`](deny.toml)).

"Stintmate" is a name of the Stintmate project and "Automobilista 2" is a trademark of its
owner; the licence grants no right to use either name except to describe the origin of the
work. Forks are welcome; please give them another name.
