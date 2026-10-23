# AMS2 connector

The Automobilista 2 connector for [Stintmate](https://stintmate.app): it reads the game's
shared memory and records sessions and laps through Pitwall, Stintmate's telemetry database,
over [Stintmate IPC](https://github.com/Stintmate-App/stintmate-ipc). It is also the reference
connector for authors who want to bring another simulator to Stintmate.

**Status:** being rewritten in Rust (version 3.0.0 in progress). Until the rewrite reaches
parity, the Python connector (2.0.0, in the Stintmate repository) is the one in use.

| Path | What |
|---|---|
| `crates/ams2-shm/` | the game's shared memory (version 14): layout, mapping, consistent snapshots |
| `crates/ams2-core/` | the logic without Windows or IPC: sessions, laps, announcements, live frame |
| `crates/ams2-platform/` | Windows helpers: workspace, logging, processes, timers |
| `crates/ams2-i18n/` | the connector's texts and the catalogs contract loader |
| `bins/ams2-recorder/` | the recorder process (50 Hz, no window, below-normal priority) |
| `bins/ams2-connector-ui/` | the control app (tray, window, supervision of the recorder) |

Documentation: [`docs/`](docs/README.md): [usage](docs/usage.md), [installer](docs/installer.md), [architecture](docs/ARCHITECTURE.md),
[what we learned about AMS2](docs/FINDINGS.md) and [writing a connector](docs/writing-a-connector.md).

## Building

Requirements on Windows: Rust (the version in `rust-toolchain.toml`, installed by rustup) with
the MSVC target and the Visual Studio Build Tools C++ workload.

```
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Licence and name

Apache License 2.0 ([`LICENSE`](LICENSE), [`NOTICE`](NOTICE)). Every dependency compiled into
the connector is permissively licensed (checked by `cargo deny`, [`deny.toml`](deny.toml)).

"Stintmate" is a name of the Stintmate project and "Automobilista 2" is a trademark of its
owner; the licence grants no right to use either name except to describe the origin of the
work. Forks are welcome; please give them another name.
