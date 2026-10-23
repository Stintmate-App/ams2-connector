# U0 UI spike: eframe/egui + tray-icon + muda (PRY-057, decision D11)

Throwaway spike in `spikes/ui/`: its own crate with its **own `[workspace]` root**, so it is not a
member of the repository workspace and the workspace's `fmt`/`clippy`/`test` never build it. Run
it from its folder's manifest: `cargo build --release --manifest-path spikes/ui/Cargo.toml`
(`--no-default-features --features wgpu` for the wgpu renderer; default is `glow`).
`ams2-u0-ui-spike <results.json> <log-file> [idle_secs]` drives itself with timers (own window,
own tray icon, own HWND only; no screenshots, no input to other windows) and writes JSON.

Measured on the driver's PC (Windows 11, 100 % scaling, dark theme), release build, 60 s idle blocks.

## Results

| Check | glow | wgpu |
|---|---|---|
| Window starts hidden | yes, **with a workaround** (see Findings 1) | same |
| Show / hide from the tray menu command | works; hidden to visible in 190 ms | 102 ms |
| Close-to-tray (`WM_CLOSE` to own window) | hidden again in 5.5 ms, process alive | same |
| Exit command while hidden | event loop stops (no forced exit) | same |
| Tray menu events while hidden (no egui frame) | 5 of 5 handled; background thread saw each in 7-35 us; log lines written; 0 egui frames ran | same (7-51 us) |
| `request_repaint` from another thread | 60 ticks, last painted counter = 60, window updated | same |
| Idle CPU, window hidden, 60 s | 15.6 ms = **0.03 %** of one core, 0 frames | 218.8 ms = **0.36 %** |
| Idle CPU, window shown but idle, 60 s | 0 ms = **0.00 %**, 0 frames | 109.4 ms = **0.18 %** |
| Graphics context while hidden | **yes**, GL context created at startup | **yes**, wgpu render state at startup |
| Working set hidden / shown | 120 / 123 MB | 188 / 193 MB |
| Release binary | **5.97 MB** | 12.45 MB |
| Dark / light | `system_theme` = Dark, visuals dark, registry `AppsUseLightTheme` = 0 (agree) | same |
| DPI | `pixels_per_point` 1.0, `native_pixels_per_point` 1.0, `GetDpiForWindow` 96 | same |

Idle CPU of the glow build is below the 15.6 ms clock granularity of `GetProcessTimes`
(one tick in 60 s); egui runs no frame while idle, with the window hidden or shown.

## Findings

1. **eframe 0.37 always shows the window after the first painted frame**, even with
   `with_visible(false)` (`epi_integration::post_rendering` calls `set_visible(true)` on the first
   frame). Left alone, the window is on screen for for a few tens of ms at startup (7-8 polls at about 15 ms). Workaround
   used and verified: create the window at `with_position([-32000, -32000])`, send
   `ViewportCommand::Visible(false)` in each of the first three frames, and when the tray asks to
   open it send `OuterPosition` then `Visible(true)`. The window is then visible only off-screen
   (rect `-32000,-32000`) for a few frames; nothing appears on screen or to the user. A taskbar
   button may blink for that moment (not measurable without looking at the shell, which the spike
   must not do): U1 should create the window with `with_taskbar(false)` until first shown, or
   verify this.
2. **The tray works while the window is hidden**: tray-icon/muda deliver the menu event through the
   tray's own window and the event handler (`MenuEvent::set_event_handler`) runs on the UI thread
   whether or not egui paints. `ctx.send_viewport_cmd` plus `ctx.request_repaint()` from that
   handler does wake a hidden eframe window and shows it (one wake-up, 13 frames in the first
   moments), so no Win32 fallback was needed.
3. **Close-to-tray**: intercept `close_requested`, send `CancelClose` and `Visible(false)`; a real
   exit must set a flag first so the same handler does not cancel it (done with an atomic).
4. **A graphics context exists the whole time the app runs**, hidden or not (eframe creates it
   with the window; nothing releases it on hide). With glow that is a plain OpenGL context and
   0.03 % CPU, 120 MB; wgpu adds 65 MB, 6.5 MB of binary and measurable idle CPU (cause not investigated, 0.2-0.4 %).
   Neither renders while idle. The cost the spec feared
   (frame time next to a VR game) is only paid while the window is shown and repainting; the
   window should be opened on demand and not kept visible during a race. The GPU context cannot be
   dropped without destroying the window; if that ever matters, U1 can close the viewport instead
   of hiding it (the process, tray and services keep running) at the cost of re-creating the
   window and its context on each Open.
5. **Not covered, stated plainly**: the physical tray right-click and the popup menu itself (the
   spike posts `WM_COMMAND` with the muda command id to the tray window, which exercises the same
   handler path but not `TrackPopupMenu`; U1's desk test must click the real menu once); DPI at
   150 % (the PC is at 100 %; winit is per-monitor DPI aware and egui reports the native scale, but
   the 150 % value was not read); a live switch of the system theme (egui follows
   `system_theme`; only the current state was read).

## Dependencies and licences

`eframe 0.37` (default features off, `default_fonts` plus one renderer), `tray-icon 0.26`,
`muda 0.21`, `raw-window-handle 0.6`, `serde_json`, `windows-sys 0.61` (the spike only; the real
crates already use `windows-sys`). `cargo tree` for the glow build: 119 crates; for wgpu: 175.
`cargo deny` is not installed here; licences were read from `cargo tree -f "{p}|{l}"`.

- Nothing GPL/LGPL/AGPL/MPL is required: every crate is MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib,
  0BSD, Unlicense or a dual/triple of these. The one GPL mention is `self_cell 1.3.0`,
  `Apache-2.0 OR GPL-2.0-only`: we take Apache-2.0.
- `unicode-ident`: `(MIT OR Apache-2.0) AND Unicode-3.0` (permissive, data licence; needs the
  notice in `NOTICE`/`about.toml`).
- `epaint_default_fonts`: `(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0` (the embedded fonts;
  permissive, notices to be included in the third-party list; disable `default_fonts` and ship a
  font to avoid them).
- The glow build adds to nothing outside permissive terms; the wgpu build adds 56 crates, all
  `MIT OR Apache-2.0`-style, one `ISC`, two `BSD-2-Clause OR Apache-2.0 OR MIT`.

## Recommendation

**eframe/egui + tray-icon + muda, with the glow renderer** (D11 confirmed). Every check passed:
hidden start (with the off-screen workaround), tray events with no frame running, cross-thread
repaint, close-to-tray, exit while hidden, system theme read, zero idle frames and about zero idle
CPU. glow is half the binary size and 65 MB lighter than wgpu with lower idle CPU, and a window
that only shows services, options and logs needs no GPU abstraction.

Native Win32 via `windows-sys` stays the fallback if U1 hits a blocker (for example the taskbar
blink in finding 1, or an unacceptable memory footprint for a tray-resident process: 120 MB
working set is mostly the GL driver and fonts; a native UI would be far smaller, not measured).

For U1: keep the off-screen start and the `CancelClose` pattern, set the exit flag before
`Close`, open the window on demand and `request_repaint` from the services' threads, and test the
real tray menu click and 150 % scaling at a desk.
