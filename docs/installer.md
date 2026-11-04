# The installer

`AMS2-Connector-<version>[-dev]-setup.exe` installs the connector for the current user (Inno
Setup 6; no administrator rights, no UAC, English only, unsigned). This page is the
contract for whoever installs it without a person in front of it: Stintmate downloads the
connector and runs this setup in the background. Fetching the setup and verifying it
(the `.sha256` file beside it, or the caller's own verification) is the caller's job; the setup
itself checks nothing about where it came from.

## What it does

- Installs into `%LOCALAPPDATA%\Programs\Stintmate AMS2 Connector\` (the folder is fixed; for
  tests, `/DIR="<folder>"` overrides it): the three programs, `LICENSE`, `NOTICE`,
  `THIRD-PARTY-NOTICES.txt` and a short `README.txt`. It has its own entry in Windows' Installed
  apps, a Start menu shortcut named like the connector (no desktop icon) and an uninstall entry
  in the Start menu; the uninstaller removes both.
- **The shortcut never starts a connector.** Stintmate starts and supervises it; a connector
  started alone would run unmanaged next to Stintmate's own instance. The shortcut runs
  `ams2-connector-ui.exe --from-shortcut` ([usage](usage.md#command-lines)): with Stintmate
  installed it opens Stintmate and exits; without it, a message says the connector needs
  Stintmate and offers https://stintmate.app. (`ams2-connector-ui.exe` started by hand without
  that option still behaves as before.)
- **Needs no workspace.** It registers the connector in the per-user machine file
  `%LOCALAPPDATA%\Stintmate\connectors\ams2.json`, which is how Stintmate finds it:
  `ams2-connector-ui.exe --register --uninstall "<unins000.exe>" [--flavour dev]` writes it
  atomically, with the path of this install's launcher and the command that removes this
  install. The file is the connector descriptor described in the connector-kit's
  [`descriptor.md`](https://github.com/Stintmate-App/connector-kit/blob/main/docs/descriptor.md). It
  does not depend on a workspace, so the connector can be installed before Stintmate, and
  Stintmate lists it as soon as it starts. `STINTMATE_CONNECTORS_DIR` replaces the folder (tests).
- **Never starts the connector** and never touches the workspace's data (logs, outbox,
  settings). Uninstalling removes the programs and the descriptor, never the data.
- A **development build** (`scripts\build-installer.ps1 -Dev`, file name `...-dev-setup.exe`)
  has its own identity (AppId, name `Stintmate AMS2 connector (dev)`, folder) and writes
  `"flavour": "dev"` into the file; Stintmate shows the version with "(dev)" after it. See "One flavour at a time".

## One flavour at a time

Both builds share the connector's id, endpoint and single-instance mutex, and there is one
`ams2.json`, so only one can be installed. Before copying anything, the setup runs
`ams2-connector-ui.exe --read-registration` (its own exe, extracted to a temporary folder) and
looks at the file:

1. **No valid file of this connector, or one that names this very install folder:** nothing to
   replace. The setup stops this install's own programs (below) and goes on.
2. **A file that names another install folder** (the other flavour, or a copy elsewhere):
   - that install's programs are stopped, with the same rules as for its own (`/MANAGED` fails
     with `connector_running`, an interactive run asks to close Stintmate);
   - when the file has an `uninstall` command and that program exists, the setup runs it (the
     progress line `{"stage":"replace"}`) and waits up to 30 s for the other install's launcher
     to disappear. Inno's uninstaller hands over to a copy of itself, hence the wait. If it is
     still there, the run fails with `replace_failed`;
   - a file with no `uninstall` (a development checkout, or any other registration without one) removes nothing:
     the setup takes the registration over (`--register` overwrites the file).
3. The setup copies the files and runs `--register`.

The uninstaller removes `ams2.json` only when its `launcher` names this install (paths compared
absolute, case-insensitive, `/` read as `\`). If the file names another install it stays and the
uninstall still succeeds.

## Command line

```
AMS2-Connector-<version>-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART ^
    /MANAGED /PROGRESS="<file>" /LOG="<file>"
```

`/VERYSILENT` (or `/SILENT`) is what makes a run **unattended**; Inno Setup's other parameters
(`/DIR`, `/LOG`, ...) work as documented by Inno Setup. The first two below are the connector's; `/LOG` is Inno Setup's own.

| Parameter | Meaning |
|---|---|
| `/MANAGED` | the caller stopped the connector and starts it again afterwards: the setup never closes anything and fails with `connector_running` when one of the connector's programs still runs |
| `/PROGRESS="<file>"` | write progress as JSON lines to that file (below) |
| `/LOG="<file>"` | Inno Setup's own log (diagnosis; its text is not a contract) |

The uninstaller (`unins000.exe` in the install folder; the command is in the registry's
`UninstallString` and in the file's `uninstall`) takes `/VERYSILENT /SUPPRESSMSGBOXES
/NORESTART /MANAGED /PROGRESS` the same way. Inno's uninstaller runs from a copy
of itself, so the original process can end **before** the `done` line is written: after it ends,
wait for the `done` line with a timeout (for example 10 s) before reading the outcome.

## Progress file

One JSON object per line, UTF-8, `\n` line ends. The setup truncates the file when it starts,
appends each line and closes the file again (so a reader sees each line as soon as it is
written). Read the **last line**: it is always the `done` line, and it is the authority on the
outcome (the process exit status below is the backup).

| Line | When |
|---|---|
| `{"stage":"prepare"}` | the setup reads the registration and checks for a running connector |
| `{"stage":"replace"}` | removing the other installation (below), only when there is one |
| `{"stage":"files","done":37,"total":100}` | copying the files (or removing them, in the uninstaller); `done` rises from 0 to 100 and `total` is always 100: it is a percentage, not bytes. A line is written whenever the percentage changes; 0 first, 100 when the files are in place |
| `{"stage":"register"}` | running `--register` (installer) |
| `{"stage":"unregister"}` | running `--unregister` (uninstaller) |
| `{"stage":"done","ok":true}` | finished |
| `{"stage":"done","ok":true,"warning":"unregister_failed"}` | uninstalled, but the descriptor could not be removed (Stintmate will find a connector that does not start: remove `%LOCALAPPDATA%\Stintmate\connectors\ams2.json`) |
| `{"stage":"done","ok":false,"reason":"..."}` | failed before anything was copied (or while copying: `disk`) |
| `{"stage":"done","ok":false,"reason":"register_failed","installed":true}` | the files are installed but `--register` failed (rare) |

A successful install, for example:

```
{"stage":"prepare"}
{"stage":"files","done":0,"total":100}
{"stage":"files","done":41,"total":100}
{"stage":"files","done":100,"total":100}
{"stage":"register"}
{"stage":"done","ok":true}
```

A failed one: `{"stage":"prepare"}` then `{"stage":"done","ok":false,"reason":"connector_running"}`.

### Reasons and exit statuses

| `reason` | Meaning | Inno Setup exit status |
|---|---|---|
| `connector_running` | one of the connector's programs (`ams2-connector-ui.exe`, `ams2-recorder.exe`, `ams2-connector-window.exe`, matched by image path inside the install folder, this one's or the other flavour's) runs and was not closed: with `/MANAGED` the setup never closes it; unattended without `/MANAGED` it only closes an unmanaged one | 7 (install), non-zero (uninstall) |
| `replace_failed` | the other installation's `uninstall` command ran (or could not run) and its launcher was still there after 30 s. Uninstall it from Windows' Installed apps, then run the setup again | 7 |
| `register_failed` | before copying: the installed registration cannot be read (`--read-registration` failed). After copying (`"installed":true`): `--register` failed | 7 before copying; 9 installed but not registered |
| `disk` | the files could not be written (disk full, a locked file, no permission), or the setup was cut short while copying | 4 |
| `cancelled` | interactive only: the person cancelled before the copy started | 2 |

Exit status 0 is success. The progress file's `reason` is the authority; the status is the backup.

### Nothing is rolled back

Every failure that can be foreseen is found **before the first file is copied**: a running
connector, an installation that cannot be removed, a registration that cannot be read. A failure
while copying (`disk`) is Inno Setup's own. If the other flavour was already removed and a later
step fails, it stays removed. If `--register` fails after the copy (rare), the files stay (a
rollback would not restore the files an upgrade replaced), the status is 9 and the done line
carries `"installed":true`: the caller may run `ams2-connector-ui.exe --register --uninstall
"<unins000.exe>"` itself from the install folder, or run the setup again.

## Interactive and silent behaviour

| Run | A connector of this installation (or the replaced one) is running |
|---|---|
| `/MANAGED` (unattended) | fails with `connector_running`; nothing is closed |
| unattended without `/MANAGED` | `ams2-connector-ui.exe --quit`; an unmanaged connector is closed. When Stintmate manages it (`--quit` exits 4) the setup fails with `connector_running`: **it never closes Stintmate** |
| interactive | the same, but for a managed connector it **asks** to close the installed Stintmate (`stintmate.exe --running` / `--quit-all`) and, while a session is recorded, offers Retry or Cancel |

### The hold: updating without closing Stintmate

Unless it runs with `/MANAGED`, the setup first asks Stintmate (the hub) to stop **only the
connector** and keep it stopped, so the question above rarely comes up. During `prepare`, after
the registration is read, the extracted `ams2-connector-ui.exe --stintmate-hold` (`--purpose
uninstall` in the uninstaller, which runs the installed build) calls `hub.connector.hold` and
waits up to 25 s until the connector's programs are gone. Its exit status decides:

| Exit | The setup does |
|---|---|
| 0 held and stopped | goes on; Stintmate does not start the connector until it is released |
| 1 held, something still runs; 2 bad arguments; 5 no Stintmate to ask (no pointer, key or hub); 6 Stintmate runs but did not answer (an older Stintmate, busy, timeout) | the older rules above (a managed connector: silent fails with `connector_running`, interactive asks to close Stintmate) |
| 7 a driving session is being recorded | interactive: "Finish it, then choose Retry" (Retry holds again); unattended: fails with `connector_running`; declining the prompt also ends the run with `connector_running` |

The hold is released with `--stintmate-release` (with `--no-start` it ends the hold without starting the connector) after `--register` (also when registering
failed), and in `DeinitializeSetup` when the run ends any other way (a failure after the hold, a
cancel, a copy error): Stintmate then starts the connector the registration names, the new build
after an update, the old one otherwise. A release that fails is only logged in the setup's log.
Stintmate ends a hold by itself after 5 minutes, so a setup that dies never leaves the
connector off. The uninstaller holds with purpose `uninstall`; if it refuses to go on, or the
person declines before any file is removed, it releases at once; once files are gone the
removal of `ams2.json` ends the hold. The progress file has no new stage (`prepare` covers the
hold).

Only processes that run from the install folder count; another copy of the connector, or any other program, is never counted nor stopped, except the one
the registration names when it
is being replaced.

## What Stintmate does around it

Install or update:

1. Stop the connector through the control pipe (`control.quit`, or stop the process it
   supervises) and wait until `ams2-connector-ui.exe`, `ams2-recorder.exe` and
   `ams2-connector-window.exe` of the install folder are gone.
2. Run the setup with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /MANAGED
   /PROGRESS="<temp file>"`, tail the progress file for the bar, wait for the process to end and
   read the last line.
3. On `ok:true`, start the connector again (read `%LOCALAPPDATA%\Stintmate\connectors\ams2.json`
   for the path). On a failure show the reason; `connector_running` means step 1 did not finish.

Uninstall: stop the connector, run `unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
/MANAGED /PROGRESS="<file>"`, read the last line.

The install folder is `%LOCALAPPDATA%\Programs\Stintmate AMS2 Connector\`; the installation's
uninstaller and version are also under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall`
(AppId `{E3A94F7B-5C18-4D2E-9B60-81F5D3C7A204}`, `_is1`). The setup keeps nothing else in the
registry.

## Building it

`scripts\build-installer.ps1 [-Dev]` from the repository: `cargo deny check`, `cargo build
--release --locked`, `cargo about generate` (`THIRD-PARTY-NOTICES.txt` from
`installer\THIRD-PARTY-NOTICES.hbs` and `about.toml`), then Inno Setup 6 (found in
`%LOCALAPPDATA%\Programs` or Program Files). Output in the git-ignored `dist\`:
`AMS2-Connector-<version>[-dev]-setup.exe` and `.sha256` (`sha256sum` format). It needs
`cargo-deny`, `cargo-about` and Inno Setup 6 installed, and says so when one is missing.
`-SkipDeny` is for offline work only.
