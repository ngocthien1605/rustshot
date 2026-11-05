# QA-CHECKLIST — RustShot (manual verification for GPU/display/UI paths)

These steps cannot run on a headless CI runner. Run them on a real Windows 11 desktop,
ideally a multi-monitor, mixed-DPI rig. Record pass/fail + notes in PROGRESS.md.

The hardware checks that need a person (listening to recordings, a second monitor, Bluetooth
and USB devices, a driver reset, sign-in at startup) are collected, with steps and pass
criteria, on the "RustShot Hardware Checks" page:
https://claude.ai/artifact/VG1gbGztE6cnHgdKji2GtC (private to the owner). Results recorded
there are copied into this file.

`dev <command>` below means `rustshot-dev <command>` (the diagnostics binary built alongside
`rustshot.exe`; entries written before review item (7) used `rustshot dev <command>`).

## M0 — Scaffold
- [ ] `rustshot.exe` launches to a tray icon with a context menu; no console window.
- [ ] Tray menu → Quit exits cleanly (process gone, mutex released).
- [ ] Launching a second instance does not create a second tray icon (single-instance).
- [ ] `rustshot-dev config` prints the loaded config and the config file path.
- [ ] First run creates `%APPDATA%\RustShot\config.toml` and the output folder.
- [ ] Logs appear under `%APPDATA%\RustShot\logs`.
- [x] Corrupt `config.toml` → app still starts: the file is renamed `config.toml.broken-<n>`
      (contents intact), a fresh default `config.toml` is written, and a startup dialog names
      both files and the parse error. *(Verified on the release build launched from `System32`
      as the Run key would; screenshot of the dialog taken.)*
- [x] `output_folder = "shots"` (relative) → silently reset to the default and persisted, no
      dialog; `output_folder = 'Z:\nope\shots'` (missing drive) → dialog "not usable", captures
      fall back to `Pictures\RustShot`, config left as typed. *(Verified the same way.)*
- [ ] Tray tooltip reads "RustShot — N hotkey(s) unavailable" when another app holds some keys,
      and updates after Settings → OK or Cancel re-registers them (code path exercised; the
      tooltip text itself has not been read back).
- [x] Settings has a taskbar button with the app icon and opens in front; the selection overlay
      takes keyboard focus even when nothing in RustShot received the last input (a delayed
      shot), so Esc reaches it. *(Driven with the action posted while a console window had
      focus: overlay foreground, a real Esc keypress cancelled it; Settings had
      `WS_EX_APPWINDOW` and the foreground.)*
- [ ] Settings → OK with a relative folder, or a folder on a missing drive, keeps the dialog
      open with a message; OK with a hotkey another app holds shows a dialog naming it.

## M1 — Capture engine (stills)
- [ ] `rustshot-dev list-monitors` lists every display with correct bounds + DPI/scale.
- [ ] `rustshot-dev grab-monitor 0 out.png` saves a pixel-correct full-monitor PNG.
- [ ] Image is correct on a 150%/200%-scaled monitor (no stretching, correct dimensions).
- [ ] Wide image where stride ≠ width×4 is not sheared (stride handled).

## M2 — Screenshots
> Already verified by automation (no manual step needed): monitor / window / region /
> all-monitors grab, auto-naming, PNG/JPEG save, and clipboard image copy — via
> `rustshot-dev {list-windows,grab-all,grab-region,grab-window,clipboard-image}` plus a
> scripted System.Drawing check. The items below are the interactive/visual ones.
- [ ] Region hotkey dims all monitors; drag shows live W×H; Enter/release captures; Esc cancels.
- [x] Selection overlay look (Region + Window screenshots, region video, region GIF): full-screen white crosshair with a dark edge follows the cursor; hovered window / dragged selection has a red border and stays bright; size label stays on top. *(Verified by screenshotting the running overlay, idle and mid-drag.)* Window screenshots use the same look (verified). Right-click cancels on release, so no context menu pops up underneath *(verified).*
- [ ] Zero-area region cancels cleanly.
- [ ] Window pick highlights the window under the cursor; click captures its bounds.
- [ ] All-monitors captures the full virtual desktop across mixed-DPI displays.
- [ ] Timer delay fires after N seconds (screenshot delay; no visible countdown by design).
- [ ] Cursor include/exclude honoured.
- [ ] Output PNG auto-named + saved to the output folder AND copied to the clipboard.
- [ ] Flyout shows a thumbnail + Annotate / Open / Open folder / Copy / Copy path / Delete, all working.
- [x] Annotate: opens the editor; Rectangle and Text tools draw in red; Text → type → Enter saves (Shift+Enter or Alt+Enter = new line, not saved; verified live); red crosshair cursor over the canvas (verified by reading the live cursor bitmap); Undo (Ctrl+Z); Save (Ctrl+S) writes `_annotated` copies without overwriting (`_annotated_2` on repeat); closing with unsaved work prompts. *(Driven end-to-end on the release build with real mouse and keyboard events; saved file inspected: annotations at full resolution in the right place.)* Still worth a human look: very large screenshots, text with accents/emoji, multi-monitor DPI.
- [x] Flyout stays while the pointer is over it (11 s hover: still open) and goes ~1 s after the
      pointer leaves; at 125 % it is drawn at 625×212 with the system font, inside the work area
      of the monitor under the pointer. *(Driven on the debug build, 2026-10-23.)*
- [x] Editor: close with no edits → closes, no prompt; draw then close → prompt; Cancel → still
      open; close again → Yes → `_annotated.png` saved and the editor closes. Toolbar scaled at
      125 %. *(Same session.)*
- [x] Four Monitor screenshots in a row into the OneDrive-synced `Pictures\RustShot` → four
      files, no temp files left, no errors.
- [x] Editor dragged to a monitor with a different scale re-lays out (`WM_DPICHANGED`); the
      flyout and the recording toolbar open on the monitor under the pointer. *(Hardware checks
      page, two monitors at different scales, 2026-10-23: pass.)*
- [ ] Protected/DRM window → near-black detected and a warning surfaced (not a silent black PNG).

## Annotation tools — arrow, highlight, pixelate, palette, redo (UI unverified)

Written on Linux, where the editor window cannot be built or run: the pixel operations and the
editor model are unit-tested, the Win32 code was only type-checked and linted for
`x86_64-pc-windows-msvc`, and the GDI render tests in `annotate/win.rs` were compiled but never
run. Every item below is still to be done by hand on Windows. Run `cargo test --workspace
--locked` there first (the render tests check arrow, highlight, pixelate and colour pixels in
the exported image).

- [ ] **Defaults unchanged.** A fresh editor draws rectangles and text in red with the 3-px line,
      as 0.1.8 did (compare an `_annotated` file from both builds of the same screenshot).
- [ ] **Toolbar** at 100 %, 125 % and 150 %: Rectangle, Text, Arrow, Highlight, Pixelate, six
      colour squares, three line samples, Undo, Redo, Save, Copy — none clipped or overlapping
      when the editor opens. The current tool's button is pressed; its colour and line have a dark
      double frame. Colour squares are blank and disabled for Pixelate; line samples are greyed
      for Highlight and Pixelate. Redo is greyed until something is undone. Dragging the editor to
      a monitor with another scale redraws the squares and samples at the new size.
- [ ] **Arrow**: shown while dragging; on release the head is at the release point, filled, and
      the shaft does not poke past the tip. A tiny drag (under ~6 px) leaves nothing. Thin /
      medium / thick change the shaft and head; colour applies. Level and vertical arrows work.
- [ ] **Highlight**: translucent while dragging; after release the canvas shows the same tint
      and the text underneath is readable. Yellow by default; another colour applies. The saved
      file matches the screen.
- [ ] **Pixelate**: a dotted frame while dragging; on release the area turns into blocks on
      screen. **Save**, open the `_annotated` file in another viewer (Photos, Paint), zoom in:
      only blocks, no readable detail. **Copy**, paste into Paint: same. The original file is
      unchanged. Try PNG and JPEG output.
- [ ] **Layering**: a rectangle drawn first, then pixelated over, stays crisp on top; a
      highlight under an arrow does not tint the arrow.
- [ ] **Undo / Redo**: buttons, `Ctrl+Z`, `Ctrl+Y`, `Ctrl+Shift+Z`. Undoing a pixelate brings the
      detail back on screen, redo hides it again. Drawing after an undo greys Redo. Redo does
      nothing mid-drag or while a text box is open.
- [ ] **Keys** `R` `T` `A` `H` `P`, `1`–`6`, `[` `]` switch tool / colour / line and the toolbar
      follows. While typing in a text box they type text. With the Japanese IME on (hiragana) they
      go to the IME; after `半角/全角` (direct input) they work. Picking a colour while typing
      recolours that text.
- [ ] **Unsaved prompt after click-away**: type text, click elsewhere on the image (commits it),
      close → the save prompt appears (before this change it did not, and the text was lost).
      Draw, Save, Undo everything, close → no prompt (nothing to save).
- [ ] **Large screenshot** (4K or all monitors): drawing, undoing and redoing pixelate and
      highlight stays responsive (the canvas is rebuilt on each change of those).

## Annotation tools 2 — black box, select/move/delete, step markers (0.1.10)

The model and the GDI render tests (black box, step markers) run on CI; the window behaviour
below is still to be tried by hand.

- [ ] **Toolbar**: Select, Rectangle, Text, Arrow, Highlight, Pixelate, Black box, Step, the six
      colours, three lines, Undo, Redo, Save, Copy — all visible, none clipped, at 100 % and at
      your usual scale (125 % / 150 %). Rectangle is still the tool pressed when the editor opens.
- [ ] **Black box** (`B`): solid black while dragging and after release; Save → the
      `_annotated` file is black there (zoom in: no detail at all). Colour and line buttons are
      greyed while it is the tool.
- [ ] **Step** (`N`): clicks place 1, 2, 3 …; the marker follows the pointer while the button
      is down. Colours: white number on red/green/blue/black, black on yellow/white. Thin /
      medium / thick change the size. Undo the last one, click again → the same number is reused.
      Numbers 10+ fit inside the circle.
- [ ] **Select** (`S`): the cursor is an arrow, and a four-way arrow over a mark. Click a
      rectangle's edge, an arrow, a text box, a highlight, a black box, a step marker → a dotted
      frame around it. Clicking inside a hollow rectangle selects what is inside it (or nothing).
      Empty space or `Esc` deselects.
- [ ] **Move**: drag a selected mark; it stays inside the picture at every edge. Dragging a
      pixelate area or black box shows the content under it until release, then covers it again
      at the new place; the saved file is covered at the new place only.
- [ ] **Delete** removes the selected mark; **Undo** brings it back in the same place. Undo also
      takes back a move and a colour change one at a time; Redo reapplies them.
- [ ] **Restyle**: with a mark selected, the toolbar shows its colour and line; clicking another
      colour / line (or `1`–`6`, `[` `]`) changes that mark only. Greyed for pixelate and black
      box. Switching back to the Rectangle tool shows the Rectangle tool's own colour.
- [ ] **Large screenshot** (4K / all monitors): moving a black box or highlight is smooth while
      dragging (the image is rebuilt once at the start and once at the end).

## M3 — Recording (MP4, video-only)
- [ ] Countdown → compact always-on-top toolbar (elapsed + Pause/Stop) → Stop finalises.
- [ ] 60 s 1080p30 monitor recording plays correctly in VLC and Windows Media Player.
- [ ] Pause/resume produces correct timing (no frozen/!sped-up segments).
- [ ] Dropped-frame count logged; memory stays bounded (ring buffer) under load.
- [ ] Read-back throttle: record a video playing full screen at 30 fps output and check the log's
      `captured` count is about wall-time × 30, not × the display refresh rate. *(Only seen on an
      idle desktop so far, where WGC barely fires anyway.)*
- [x] Target window closes mid-record → recording stops and finalises a playable file. *(Verified
      on the release build: `dev record-window` on a test console window killed at 5 s returned
      "ENDED EARLY … the captured window was closed" within 250 ms and `probe-mp4` decoded 121
      samples / 4.0 s; in the tray app, a window picked in the overlay and then killed made the
      toolbar disappear within 0.7 s with no Stop press, a "recording ended early" notice naming
      the saved file, and a playable 3 s MP4.)*
- [x] Source window resized mid-record → log shows `frame pool recreated at WxH` for each size
      change; the file keeps its initial dimensions and probes OK. *(Verified with
      `dev record-window` while shrinking then growing the window.)*
- [ ] GPU device removed/reset mid-record → recording ends early with "the graphics device was
      lost" and a playable file. *(Not provokable here; the device poll and the callback's
      failure counter are unit-tested only.)*
- [x] Concurrency gate: while the record overlay or countdown is up, a screenshot hotkey and
      Settings are refused (`capture already in progress` / `busy; settings dialog not opened`);
      Stop during the countdown cancels it (`recording countdown cancelled`), nothing starts,
      and the gate is released. *(Driven on the release build via the dispatch window; log and
      output folder checked.)*
- [x] Quit mid-recording finalises the file: `recording saved to …`, loop exits cleanly, and
      `dev probe-mp4` decodes the result (125 samples, 624×500). *(Verified the same way.)*
- [ ] Panic containment: unit-tested (`panic_guard`); a real callback panic surfacing as the
      one-time dialog + `logs\panic.log` has not been provoked live.

## M4 — Audio
- [x] Mic-only, system-only, and mic+system each mux correctly. *(2026-10-23, by ear on the
      hardware checks page: mic-only and mic+system recordings play, system audio clear and steady
      throughout, mic audible but quiet: it was the Windows default input (the laptop's), not
      the earphone mic the tester spoke into, which is what the Settings microphone picker is for.
      Earlier: system-only verified by
      signal: `dev record-audio-test 0 10` → `probe-mp4` audio 9.11 s vs video 9.07 s, 1 kHz
      tone found, no clock corrections logged. Mic and mic+system verified in memory only with
      `dev audio-probe --mic` / `--system --mic`: both sources open, full-rate frames, non-zero
      level; nothing written to disk. A mic recording to MP4 still needs a human listen.)*
- [ ] A/V stays in sync (±1 frame drift over 3 min). *(2026-10-23: a 102 s mic+system recording
      stayed in sync to the end by ear; log: audio 101.9 s vs 3058 frames at 30 fps = 101.9 s,
      no clock corrections. The full 3 minutes is still to do.)*
- [x] `microphone_device` that matches nothing → default microphone used plus the note "the
      microphone "…" was not found"; a partial name selects the matching device. *(Verified
      with `dev audio-probe --mic --mic-device …`.)*
- [ ] "No audio device" / a source that cannot open → the other source still records, the
      video is saved, and a dialog after the recording lists what is missing. *(Per-source setup
      and the dialog text are unit-tested; not provoked live: no way to make the mic fail here.)*
- [x] Settings → Microphone lists "System default (…)" plus each connected microphone; choosing
      one and pressing OK writes its name to `microphone_device` and the next recording uses it;
      a chosen microphone that is unplugged shows as "not connected" and stays selected. *(Choice
      list and matching unit-tested; the dialog renders with the drop-down filled (`dev
      settings-selftest`: 2 entries here). Driven by hand 2026-10-23 on the master build: with
      the Realtek input as Windows default, choosing the built-in Intel array wrote its name to
      `microphone_device` and the next recording used it (log: `audio source microphone:
      "マイク配列 (…)"`); the tester confirmed the picker behaves as described. The
      "not connected" entry and a Bluetooth headset mic were not exercised.)*
- [ ] Unplugging a USB/Bluetooth microphone mid-recording → the rest of the take has no mic
      sound, the file is fine, and the dialog says the microphone stopped. *(2026-10-23: the
      device that dropped was the AirPods' output, so it was **system audio** that stopped
      (`0x88890004`): file saved and playable, the dialog named system audio, the microphone (the
      built-in array, still the Windows default) kept recording. Device loss mid-recording
      therefore works; the microphone case needs the picker set to the headset's mic.)*
- [ ] A 24-bit / 5.1 device records audible sound (decode and downmix are unit-tested; this
      machine's devices are all 32-bit float stereo).
- [ ] Record with a Bluetooth headset microphone and listen for dropouts. The mixer drops a
      source's audio that arrives more than ~50 ms late (to keep it in sync), so a device that
      delivers in long bursts would sound chopped rather than delayed; the knob is
      `STARVE_LIMIT` in `recorder/src/audio.rs`. *(2026-10-23: not yet tested; the take recorded
      with AirPods connected used the built-in microphone array, the Windows default input, not
      `Headset (AirPods Max 2)`. Retest with the picker, and check that opening the headset's
      mic (which switches a Bluetooth headset to its hands-free profile) doesn't kill a
      system-audio loopback on its `Headphones` endpoint.)*

## M5 — GIF
- [ ] Short region recording exports an optimised GIF that opens in a browser.
- [x] Dimension/length caps enforced with a clear warning. *(Length cap: with `gif_max_seconds =
      5` the toolbar closed at ~5 s with the notice "it reached the 5-second limit for GIF
      recordings" and the GIF plays ~5 s; log 69 frames over 75 slots, 5.1 s wall. Hardware
      checks page, 2026-10-23.)*
- [x] GIF playback length matches the time recorded: `dev record-gif 0 6` → 5.73 s of playback
      from 6.0 s wall (first frame ~0.3 s in), 19 distinct frames over 86 slots.

## M6/M7
- [ ] Settings UI edits persist and take effect.
- [x] Hotkeys section: editing a shortcut field re-registers that hotkey on OK; a blank field
      unbinds it; an invalid entry or a duplicate combination is reported and the dialog stays
      open; "Restore default hotkeys" resets all six fields. *(Driven end-to-end against the
      release build with UI automation: rebinding Monitor screenshot to Ctrl+Alt+F8 in-session
      made the new key capture a screenshot and the old key stop; invalid/duplicate entries named
      the offending row and kept the dialog open; Restore defaults + OK reset the config.)*
- [x] Hotkeys "Modify" button: capture dialog records the pressed combination (incl.
      `PrintScreen`), Alt-combos; Esc leaves the field unchanged, Backspace clears it, tapping
      Alt alone does nothing. *(Verified with real key events, including Shift+PrintScreen with
      Screenpresso running.)* Hotkeys are suspended while Settings is open and restored after
      Cancel. Still worth a human check: the Win-key combos, and PrintScreen with Windows'
      "Use the Print screen key to open screen capture" setting turned on.
- [x] Settings → Browse: the folder picker is modal to the dialog (dialog disabled while it is
      open, enabled again after it closes). Settings renders themed (Common Controls v6).
- [x] Settings + capture dialog templates render without error — auto-verified by
      `rustshot-dev settings-selftest` (both must report OK, not a `-1` result).
- [ ] Run-at-startup toggle adds/removes the `Run` key (opt-in).
- [ ] Installer produces a working Start-menu app on a clean Windows 11 VM; portable zip runs.
- [ ] Following `docs/MANUAL-GUIDE.md` (or `MANUAL-GUIDE.vi.md`) from a fresh download on a
      clean machine matches what RustShot shows: menu and button names, defaults, file
      locations, messages.
