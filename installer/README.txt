Stintmate AMS2 Connector
========================

The Automobilista 2 connector of Stintmate (https://stintmate.app): it reads the game's shared
memory and records your sessions and laps through Pitwall.

Stintmate starts and stops the connector for you. The Start menu shortcut opens Stintmate (or, if
it is not installed, tells you where to get it); it never starts a connector by itself. The
connector registers itself for Stintmate when it is installed (the file
%LOCALAPPDATA%\Stintmate\connectors\ams2.json), whether or not Stintmate is set up yet.

Programs in this folder:
  ams2-connector-ui.exe       the control app (tray icon, settings window launcher)
  ams2-recorder.exe           the recorder (reads the game, below normal priority)
  ams2-connector-window.exe   the connector's window

Your data (logs, pending laps, settings) is never stored here: it lives in your Stintmate
workspace, in connectors\ams2\. Uninstalling removes the programs and the connector's
registration, never your data.

Licence: Apache License 2.0 (LICENSE, NOTICE). Open source components: THIRD-PARTY-NOTICES.txt.
Source and documentation: https://github.com/Stintmate-App/ams2-connector
