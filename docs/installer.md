# The installer

`AMS2-Connector-<version>[-dev]-setup.exe` installs the connector for the current user (Inno
Setup 6; no administrator rights, no UAC, English only, unsigned for now). This page is the
contract for whoever installs it without a person in front of it: Stintmate downloads the
connector and runs this setup in the background (PRY-043). Fetching the setup and verifying it
(the `.sha256` file beside it, the signed release manifest) is the caller's job; the setup
itself checks nothing about where it came from.

## What it does

- Installs into `%LOCALAPPDATA%\Programs\Stintmate AMS2 Connector\` (the folder is fixed; for
  tests, `/DIR="<folder>"` overrides it): the three programs, `LICENSE`, `NOTICE`,
  `THIRD-PARTY-NOTICES.txt` and a short `README.txt`. It has its own entry in Windows' Installed
  apps and an uninstall entry in the Start menu.
- **No shortcut to start the connector.** Stintmate starts and supervises it; a shortcut would
  run it unmanaged next to Stintmate's own instance. (`ams2-connector-ui.exe` still works when
  started by hand.)
- Registers the connector with Stintmate: `ams2-connector-ui.exe --register [--workspace <dir>]`
  writes `<workspace>\connectors\ams2\connector.json`, which is how Stintmate finds it.
- **Never starts the connector** and never touches the workspace's data (logs, outbox,
  settings). Uninstalling removes the programs and the descriptor, never the data.
- A **development build** (`scripts\build-installer.ps1 -Dev`, file name `...-dev-setup.exe`)
  has its own identity (AppId, name `Stintmate AMS2 Connector (dev)`, folder), so installing it on
  a test folder never replaces or removes a real installation.

## Command line

```
AMS2-Connector-<version>-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART ^
    /WORKSPACE="<dir>" /MANAGED /PROGRESS="<file>" /LOG="<file>"
```

`/VERYSILENT` (or `/SILENT`) is what makes a run **unattended**; Inno Setup's other parameters
(`/DIR`, `/LOG`, ...) work as documented by Inno Setup. The four below are the connector's.

| Parameter | Meaning |
|---|---|
| `/WORKSPACE="<dir>"` | register the connector in that workspace. Without it, the one named by `STINTMATE_WORKSPACE` or by `%APPDATA%\Stintmate\settings.json` |
| `/MANAGED` | the caller stopped the connector and starts it again afterwards: the setup never closes anything and fails with `connector_running` when one of the connector's programs still runs |
| `/PROGRESS="<file>"` | write progress as JSON lines to that file (below) |
| `/LOG="<file>"` | Inno Setup's own log (diagnosis; its text is not a contract) |

The uninstaller (`unins000.exe` in the install folder; the command is in the registry's
`UninstallString`) takes `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /MANAGED /PROGRESS` the same
way. It uses the workspace the installation registered in.

## Progress file

One JSON object per line, UTF-8, `\n` line ends. The setup truncates the file when it starts,
appends each line and closes the file again (so a reader sees each line as soon as it is
written). Read the **last line**: it is always the `done` line, and it is the authority on the
outcome (the process exit status below is the backup).

| Line | When |
|---|---|
| `{"stage":"prepare"}` | the setup checks for a running connector and (unattended) for a workspace |
| `{"stage":"files","done":37,"total":100}` | copying the files (or removing them, in the uninstaller); `done` rises from 0 to 100 and `total` is always 100: it is a percentage, not bytes. A line is written whenever the percentage changes; 0 first, 100 when the files are in place |
| `{"stage":"register"}` | running `--register` (installer) |
| `{"stage":"unregister"}` | running `--unregister` (uninstaller) |
| `{"stage":"done","ok":true}` | finished |
| `{"stage":"done","ok":true,"warning":"unregister_failed"}` | uninstalled, but the descriptor could not be removed (Stintmate will find a connector that does not start: remove `<workspace>\connectors\ams2\connector.json`) |
| `{"stage":"done","ok":false,"reason":"..."}` | failed; nothing is left half-installed (see "Failure" below) |

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
| `connector_running` | one of the connector's programs (`ams2-connector-ui.exe`, `ams2-recorder.exe`, `ams2-connector-window.exe`, matched by image path inside the install folder) runs, and with `/MANAGED` the setup does not close it; or it could not be closed | 7 (install), non-zero (uninstall) |
| `no_workspace` | unattended and no workspace: `/WORKSPACE` names a missing folder, or none is given and neither `STINTMATE_WORKSPACE` nor the pointer file exists (checked before copying anything); or `--register` found the folder unusable (exit 3: not a workspace, inside OneDrive, ...) | 7 when checked before copying; 5 when `--register` fails after copying |
| `register_failed` | `--register` failed for another reason (exit 1: the descriptor cannot be written) | 5 |
| `disk` | the files could not be written (disk full, a locked file, no permission), or the setup was cut short while copying | 4 |
| `cancelled` | interactive only: the person cancelled before the copy started | 2 |

Exit status 0 is success. Inno Setup reports an `Abort` raised during the installation as 5
("cancelled during the installation"); the progress file's `reason` tells the real cause, so
**decide on the progress file**, not on the status.

### Failure leaves nothing behind

A failed `--register` after the files were copied **rolls the installation back** (Inno Setup
removes what it copied; whether an upgrade also gets its replaced files back is to be verified in the review). Reasoning: a half-installed connector
that Stintmate cannot find would never start and nobody would notice; failing cleanly lets the
caller retry. The cheap failures (`connector_running`, a missing workspace) are all checked
before the first file is touched. After a rolled-back upgrade over a running installation the
previous version is what remains. For the case of `no_workspace` with files wanted anyway, run
the setup interactively: it installs and tells the person to finish Stintmate's first run
(the connector registers itself when it starts).

## Interactive and silent behaviour

| Run | A connector of this installation is running |
|---|---|
| `/MANAGED` (unattended) | fails with `connector_running`; nothing is closed |
| unattended without `/MANAGED` | `ams2-connector-ui.exe --quit`; when Stintmate manages it (`--quit` exits 4), the installed Stintmate is closed through its documented `stintmate.exe --running` / `--quit-all` **without asking** (an idle Stintmate; a busy one, with a session being recorded, makes the setup fail with `connector_running`) |
| interactive | the same, but it **asks** before closing Stintmate and, while a session is recorded, offers Retry or Cancel |

Only processes that run from the install folder count; another copy of the connector, or the
older Python one, is never counted nor stopped. Interactive, when `--register` exits 3 (no
workspace): an information box ("open Stintmate and finish its first run; the connector
registers itself when it starts") and the installation succeeds.

## What Stintmate does around it

Install or update:

1. Stop the connector through the control pipe (`control.quit`, or stop the process it
   supervises) and wait until `ams2-connector-ui.exe`, `ams2-recorder.exe` and
   `ams2-connector-window.exe` of the install folder are gone.
2. Run the setup with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /MANAGED /WORKSPACE="<its own>"
   /PROGRESS="<temp file>"`, tail the progress file for the bar, wait for the process to end and
   read the last line.
3. On `ok:true`, start the connector again (read `connector.json` for the path). On a failure
   show the reason; `connector_running` means step 1 did not finish.

Uninstall: stop the connector, run `unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
/MANAGED /PROGRESS="<file>"`, read the last line.

The install folder is `%LOCALAPPDATA%\Programs\Stintmate AMS2 Connector\`; the installation's
uninstaller and version are also under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall`
(AppId `{E3A94F7B-5C18-4D2E-9B60-81F5D3C7A204}`, `_is1`). The workspace the installation
registered in is remembered under `HKCU\Software\Stintmate AMS2 Connector\Installs` (value name:
the install folder) for the uninstaller.

## Building it

`scripts\build-installer.ps1 [-Dev]` from the repository: `cargo deny check`, `cargo build
--release --locked`, `cargo about generate` (`THIRD-PARTY-NOTICES.txt` from
`installer\THIRD-PARTY-NOTICES.hbs` and `about.toml`), then Inno Setup 6 (found in
`%LOCALAPPDATA%\Programs` or Program Files). Output in the git-ignored `dist\`:
`AMS2-Connector-<version>[-dev]-setup.exe` and `.sha256` (`sha256sum` format). It needs
`cargo-deny`, `cargo-about` and Inno Setup 6 installed, and says so when one is missing.
`-SkipDeny` is for offline work only.
