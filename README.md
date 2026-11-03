# AMS2 connector

The Automobilista 2 connector for [Stintmate](https://stintmate.app): it reads the game's
shared memory and records sessions and laps through Pitwall, Stintmate's telemetry database,
over [Stintmate IPC](https://github.com/Stintmate-App/stintmate-ipc). It is also the reference
connector for authors who want to bring another simulator to Stintmate.

**Status:** released (version 3.4.0). It is a rewrite in Rust of an earlier Python connector (2.0.0), which it replaces entirely (that program is not in this repository). It is installed by its own setup, per user, and Stintmate starts and
supervises it ([installer](docs/installer.md)). Its Start menu shortcut opens Stintmate, or tells you where to get it; it never starts a connector alone.

| Path | What |
|---|---|
| `crates/ams2-shm/` | the game's shared memory (version 14): layout, mapping, consistent snapshots |
| `crates/ams2-core/` | the logic without Windows or IPC: sessions, laps, announcements, live frame |
| `crates/ams2-platform/` | Windows helpers: workspace, logging, processes, timers |
| `crates/ams2-i18n/` | the connector's texts and the catalogs loader |
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

## Releases

A release of tag `vX.Y.Z` carries two files:

| Asset | What it is |
|---|---|
| `AMS2-Connector-X.Y.Z-setup.exe` | the installer ([installer](docs/installer.md)) |
| `AMS2-Connector-X.Y.Z-setup.exe.sha256` | its SHA-256 in `sha256sum` format; check with `sha256sum --check AMS2-Connector-X.Y.Z-setup.exe.sha256` |

The release notes are the version's section of [`CHANGELOG.md`](CHANGELOG.md). Tags with a
`+` suffix (`v3.2.3+docs.1`) mark documentation-only changes: they have no release and no assets.

To publish one, a maintainer runs the `release` workflow by hand (Actions, `release`, Run
workflow) from `main`, with the `tag` to release:

- The tag must exist, be an ancestor of `main`, equal `v` + the workspace version in
  `Cargo.toml`, and have its `## <version>` section in `CHANGELOG.md`. A tag with `+` is refused.
- The workflow builds the tag's tree (not `main`'s), runs the same checks as `ci` (fmt, clippy,
  test, cargo-deny), builds the installer of the release flavour (`scripts/build-installer.ps1`),
  uploads the assets to a draft release, downloads them again and compares them with what it built.
- With `dry_run` (the default) it stops there: the draft is named `dry-run-<tag>` under the tag
  `dry-run-<version>`, the hashes are printed (next to those of the tag's published release, when
  there is one; a rebuilt installer need not be byte-identical), and the draft is deleted.
  Without it, the draft is published as the release of the tag. A tag that already has a published
  release is refused.

The logic that can run on its own is in `scripts/` (`release-check.sh`, `release-notes.sh`,
`release-report.sh`; bash, git and the GitHub CLI).

## Contributing

Issues are welcome. Pull requests are welcome too, but there is no promise that they are
reviewed or merged. For a larger change, open an issue first. Report a security problem
privately, as described in [`SECURITY.md`](SECURITY.md).

## Licence and name

Apache License 2.0 ([`LICENSE`](LICENSE), [`NOTICE`](NOTICE)). Every dependency compiled into
the connector is permissively licensed (checked by `cargo deny`, [`deny.toml`](deny.toml)).

"Stintmate" is a name of the Stintmate project and "Automobilista 2" is a trademark of its
owner; the licence grants no right to use either name except to describe the origin of the
work. Forks are welcome; please give them another name.
