# DECISIONS — RustShot

Every non-obvious choice + a one-line rationale. Newest at top within each phase.

## Global

- **Clean-room.** No Screenpresso code, resources, icons, strings, or layouts are copied. We
  reproduce *capabilities* only, using the RE analysis as architecture/UX inspiration.
- **Toolchain pinned to an exact release in `rust-toolchain.toml`** (`1.98.1`, MSVC, x64), not
  `stable`. The gate runs `clippy -D warnings`, so a new stable with new lints would turn CI red
  on unchanged code, and a release would be built by whatever compiler shipped that week. CI
  and the release workflow install the toolchain *named in that file* (one source of truth);
  bumping it is a deliberate one-line change that gets its own gate run. Separately, an `msrv`
  CI job checks the workspace with `rust-version` (1.90) so that promise stays true (verified
  locally with 1.90.0). Verified on this box: Rust 1.98.1, VS2022 BuildTools MSVC 14.44,
  Windows SDK 10.0.26100.
- **Releases are built by CI, not by hand** (`.github/workflows/release.yml`). Pushing a `v*`
  tag re-runs the whole gate (a tag push gets no CI run of its own), checks the tag equals the
  workspace version, runs `scripts/build-portable.ps1` (the same script CI runs on every push,
  so packaging is exercised continuously), writes a `sha256sum`-format checksum, and publishes
  both files to a GitHub release whose notes are the annotated tag's message (minus its
  title line). A release that
  already exists (made by hand) keeps its notes and gets its assets replaced. The MSI is not
  built there: WiX v3 isn't guaranteed on `windows-latest`.
- **Crate versions verified against crates.io on 2026-10-22** and centralised in the workspace
  `Cargo.toml`. Notables vs the prompt's indicative versions: `windows` 0.62.2 (not 0.58),
  `windows-capture` 2.0.1 (not ~1.5), `tray-icon` 0.26, `global-hotkey` 0.9, `eframe` 0.37.
  Exacts captured in `Cargo.lock`.
- **No `panic = "abort"`** in release, and panics are actively contained (`app/src/panic_guard.rs`):
  every Win32 callback body (window/dialog procedures, the hotkey handler, the WGC frame
  callback) runs under `catch_unwind`, because a panic unwinding across an `extern "system"`
  boundary aborts the process — for a tray app that means the icon just vanishes. A caught panic
  is logged, shown once in a dialog, and the callback returns a neutral value. A process-wide
  hook also writes every panic synchronously to `logs/panic.log`, since the normal file logger
  is asynchronous and may never flush before an abort. Release keeps line tables in the `.pdb`
  (`debug = "line-tables-only"`) so those locations are symbolizable.
- **Failures are surfaced, not just logged** (`app/src/notify.rs`). Any failure the user
  triggered or would otherwise never learn about — a screenshot or recording that did not
  happen, a clipboard copy or delete that failed, settings that did not save, hotkeys Windows
  refused — goes through `notify::{error,warn,notice,fatal}` as well as `tracing`. The seam is
  a plain top-most message box today: `tray-icon` 0.26 has no balloon API, and a custom toast is
  more than the seam needs; the API makes no modality promise so a toast can replace it later
  without touching call sites. Rule for callers on the UI thread: notify *before* releasing the
  `busy` gate, because the box pumps messages and a hotkey pressed meanwhile would otherwise
  start a capture nested inside it. Exception: hotkeys another app already holds are reported at
  startup only in the tray tooltip + log (a permanent condition on that machine should not pop a
  dialog on every login); Settings → OK reports them in a dialog because the user just chose them.
- **A broken `config.toml` never blocks startup** (`Settings::load_or_recover`). A file that is
  not valid TOML is renamed to `config.toml.broken-<unix-secs>` (so hand-written hotkeys can be
  recovered), defaults take over, a fresh file is written, and a startup dialog names both
  files; an *unreadable* file (I/O error) is left alone and defaults apply for the session only.
  Within a valid file, a shortcut string that does not parse deserializes as *unbound* instead of
  rejecting the whole file (trade-off: a typo shows up as a dead key, not an error; the log
  names it). A *relative* output folder is reset to the default by `sanitize()` — it would
  resolve against `System32` when launched from the `Run` key — and the Settings dialog refuses
  one on OK; an absolute folder that cannot be created falls back to the default with a dialog.
- **Edition 2021** (not 2024): the prompt allows either; 2021 is the broadly-stable choice and
  nothing here needs 2024-only features. Revisit if a dependency requires 2024.

## M0 — Scaffold

- **Workspace with 5 crates** exactly per the prompt's layout. `recorder` and the Windows
  capture backend start as typed stubs so the workspace stays green before M1/M3.
- **Config format: TOML** at `%APPDATA%\RustShot\config.toml` via `directories`. Serde with
  `#[serde(default)]` on every field so older/partial config files load forward-compatibly.
- **Single-instance: named mutex** (`CreateMutexW` + `GetLastError == ERROR_ALREADY_EXISTS`)
  via the `windows` crate — the prompt's recommended approach, no extra dependency.
- **Dev CLI** (`clap`) lets GPU/display paths be exercised on a real desktop without the tray
  UX. Since review item (7) it is its own console binary, `rustshot-dev`, built from the same
  library as the tray app (see M6); it never starts the tray.
- **Hotkey model in `config`** as plain data (`mods: Vec<Modifier>`, `key: String`) so parsing
  and conflict detection are pure and unit-testable, independent of `global-hotkey`.
- **Dev commands are subcommands** (`rustshot-dev grab-monitor 0 out.png`) rather than the
  prompt's literal `--dev` prefix — clap models them cleanly this way. Same intent. (Until
  review item (7) they were `rustshot dev …` inside the tray binary.)

## M1 — Capture engine (stills)

- **Raw WGC via `windows` 0.62 for `grab()`, not `windows-capture`.** We control stride, DPI,
  threading, and teardown directly; the WinRT projections are stable. The `windows-capture`
  crate's value is its Media-Foundation `VideoEncoder` (recording), so its evaluation is
  **deferred to M3** — and gated on whether its encoder accepts externally-supplied frames so
  `CaptureBackend` stays the seam. The prompt explicitly permits this fallback.
- **Per-grab D3D device + capture item + frame pool.** Keeps `WindowsBackend` holding no COM
  state (trivially `Send`); costs tens of ms, inside the 150 ms still budget. Streaming (M3)
  will keep a device alive instead.
- **Per-Monitor-V2 DPI awareness set programmatically** at the first line of `main()`
  (`SetProcessDpiAwarenessContext`), before the tray's hidden window exists. A manifest-based
  declaration is more robust and is deferred to M6/M7 with the icon resource.
- **WinRT apartment:** `RoInitialize(RO_INIT_MULTITHREADED)` in `WindowsBackend::new()`;
  S_FALSE / RPC_E_CHANGED_MODE tolerated. The free-threaded frame pool needs no DispatcherQueue.
- **Frame delivery:** `CreateFreeThreaded` pool + a `FrameArrived` handler that signals an mpsc
  channel; the grab waits on the channel with a 2 s hard deadline, then `TryGetNextFrame`. No
  busy-poll, no hang.
- **Stride = staging texture `RowPitch`** (may exceed `width*4`); `imaging::frame_to_rgba`
  honours it. On this dev box 1920×1080 yields `RowPitch == width*4` (no padding), so the
  padding path is exercised by **unit tests**, not this hardware.
- **Region/Window/VirtualDesktop** share the same `grab_item` pipeline: Region = grab its
  monitor + `Rect::to_local`/`clamp_to` + `Frame::crop`; VirtualDesktop = grab each monitor and
  `Frame::blit_into` a bounding-box buffer. Wired now; exercised from the UI in M2.
- **Near-black (DRM) detection** is a pure `imaging::is_near_black` heuristic; the app *warns*
  but still saves — a legitimately black screen is still a valid capture.

## M2 — Screenshots complete

- **Overlay = raw Win32, a single window over the virtual-desktop bounding box, painting a
  *frozen* capture.** No winit/eframe (avoids a second event loop fighting the tray's
  `GetMessageW` pump; winit 0.31 is beta). No `WS_EX_LAYERED` transparency: we freeze the
  desktop first (one `grab(VirtualDesktop)`), dim it with a 1×1 AlphaBlend, and restore the
  selection/hover region undimmed. The shot is cropped from the frozen frame, so the overlay
  can never leak into it and there is no hide-then-recapture race.
- **Action bus (`dispatch.rs`).** `MenuEvent`/`GlobalHotKeyEvent` handlers must be
  `Fn + Send + Sync + 'static`, so they can't hold app state; they `PostMessage` a `repr(u32)`
  `Action` to a hidden `HWND_MESSAGE` window whose proc owns all state via a thread-local
  `AppState`. A `busy` flag drops a second trigger while an overlay is open. HWND isn't `Send`,
  so handlers carry it as `isize` and rebuild it on the UI thread (where they actually fire).
- **Pure interaction core (`overlay/state.rs`, `hotkeys.rs`).** The selection state machine and
  the hotkey→`Code` table are Win32-free and unit-tested; the `win.rs`/`register` layers are
  thin plumbing. This is what keeps M2 honest on a machine where the overlay itself can't be
  scripted.
- **Window pick shares the overlay.** Click-without-drag (< 4 px) selects the top-most window
  under the cursor (`EnumWindows` z-order, first containing match); result is a clean
  `grab(Window)` recapture, falling back to a frozen-crop of the window bounds on error.
- **Capture flow never holds the `AppState` borrow across the nested overlay loop.** It clones
  the few needed settings out, runs with a fresh (zero-sized) `WindowsBackend`, then re-enters
  only to stash the flyout and clear `busy` — so re-entrant actions during an open overlay are
  safe.
- **Delay/timer off the UI thread.** A worker thread sleeps then posts `ExecuteDelayedShot`;
  the UI pump never blocks (per-second countdown UI is deferred — it belongs with the recording
  toolbar in M4).
- **Main thread is COM STA** (`CoInitializeEx`) for shell verbs / future file dialogs; the
  capture backend's `RoInitialize(MTA)` then returns `RPC_E_CHANGED_MODE`, tolerated — WGC's
  free-threaded pool is apartment-agnostic.
- **Hotkey registration failures are warnings, not fatal.** On Windows 11 `PrintScreen` (and
  often its combos) are held by the Snipping Tool / other tools; we log and continue, and the
  tray menu always provides the same actions.
- **`arboard` needs its `image-data` feature re-enabled** on the `app` dependency (the workspace
  pins it `default-features = false`), or `set_image` wouldn't exist.
- **Flyout is Win32, non-modal, StretchDIBits thumbnail.** It stores a heap-stable `Box` so its
  window proc can reach it by pointer; buttons post `Flyout*` actions back through the dispatch
  window, keeping all mutation on the UI thread.

## M3 — Recording (MP4, video-only)

- **Raw `IMFSinkWriter`, not `windows-capture`.** The decisive constraint is M4: the muxer must
  accept an AAC audio stream beside the video. `IMFSinkWriter::AddStream` does that natively, so
  starting here avoids a rewrite when audio lands. Input type is `MFVideoFormat_RGB32` (== our
  BGRA) and the SinkWriter inserts the color converter to NV12 for the H.264 encoder.
- **Continuous capture lives in `capture-core` (`CaptureStream`)**, reusing the M1 D3D device
  and the extracted `wgc::frame_to_cpu` helper; `recorder` owns only the MF encode + pacing.
  The staging copy runs on the WGC pool threads behind a `Mutex<D3dDevice>` (the D3D immediate
  context is not thread-safe).
- **Stream failure model: poll the device, never a frame clock.** WGC delivers a frame only
  when the content changes, so "no frame for N seconds" is normal on an idle desktop and must
  not be treated as a stall. What the stream *does* report: the GPU→CPU copy failing repeatedly
  (a removed/reset D3D device never recovers) sends one `StreamEvent::Error` after a bounded run
  of failures (logged sparsely, not at refresh rate); the encoder loop additionally polls
  `CaptureStream::check_health` once a second, which asks the device for its removed-reason and,
  for window targets, `IsWindow` — WGC was observed **not** to raise `GraphicsCaptureItem.Closed`
  for a window killed mid-capture, so the poll is the path that actually ends such a recording.
  On any early end the sink is still finalized and the recorder returns
  `RecorderError::EndedEarly { path, reason }` so the user is told where the partial file is;
  the toolbar's 500 ms tick posts `RecordPoll` so the app collects a self-ended recording
  instead of counting over it. Source resizes recreate the frame pool at the new content size
  (the documented sample pattern, done inside `FrameArrived` after the frame is closed); the MP4
  sink keeps its initial dimensions, so a grown source is cropped and a shrunken one is padded
  with black (`pack_bgra` zero-fills — `MFCreateMemoryBuffer` memory is uninitialized).
- **CFR re-emission on the monotonic clock.** WGC sends nothing while the screen is static, so a
  write-on-arrival file would freeze/jump. A dedicated encoder thread keeps the latest frame and,
  each iteration, writes enough copies to make the emitted frame count equal
  `active_elapsed / frame_dur`. This both fixes static-screen smoothness and makes the output's
  duration track real time instead of drifting with `sleep` latency (an early sleep-per-frame
  version produced a 3 s file for a 4 s recording — ~33 % fast). The clock is one pure type
  shared with the GIF recorder (`recorder/src/clock.rs`), and frame `n` is stamped
  `n × 10^7 / fps` from its index, not by summing a rounded duration (30 fps rounded to
  333 333 units would slip ~36 ms an hour against the audio track).
- **Read back at most `fps` frames a second.** WGC fires at the display's refresh rate (up to
  144 Hz), and every copy is a full-surface GPU→CPU read (~4.7 GB/s for 4K at 144 Hz) of which
  the encoder keeps only the newest per tick. `CaptureStream::start` takes `max_fps`: a frame
  arriving sooner than `1/fps` after the last copy is *held* (returned to the pool when a newer
  one replaces it) and copied once due, either by the next arrival or by `CaptureStream::pump`,
  which the encoder calls every tick. The pump is essential: on a static screen WGC sends nothing
  more, so a held frame — the last change — would otherwise never be delivered. The decide-and-
  copy runs under one lock, so frames are sent in order. The staging texture is cached on the
  device and reused while the surface size is unchanged.
- **Pause = stop the clock.** Paused wall-time is subtracted from `active_elapsed`, so the index
  doesn't advance and the output simply omits the paused span; resume continues seamlessly.
- **Even dimensions + top-down stride.** H.264 rejects odd width/height, so dims are rounded down
  to even from the first captured frame. `MF_MT_DEFAULT_STRIDE` is set **positive** (`w*4`) to
  declare top-down data — verified by decoding frame 0 and confirming it isn't vertically
  flipped versus a live grab (the advisor flagged this as "verify, don't trust").
- **Finalize-on-drop.** Without `IMFSinkWriter::Finalize` there is no `moov` box and the file is
  unplayable, so `RecorderHandle::drop` (and `stop`) always finalize.
- **Encoder thread apartment is MTA** (`CoInitializeEx(MULTITHREADED)`), satisfying both Media
  Foundation and the WinRT WGC projections; the UI thread stays STA.
- **Verification via `IMFSourceReader`** (`dev probe-mp4`): it is the same decode stack Windows
  Media Player uses, so a successful decode + frame-0 PNG is the headless proxy for "plays in a
  real player" (actual VLC/WMP playback stays a manual-QA item).
- **Toolbar/countdown reuse the flyout window pattern** (small topmost Win32 tool window,
  `SetTimer` ticks, buttons post `Record*`/`StopRecording` actions). `AppState` gains
  `recording: Option<Recording>` and `pending_record`.

## M4 — Audio

- **Raw WASAPI for both mic and system, not cpal.** cpal 0.18 can't do loopback, so one raw
  WASAPI code path covers both: system = `AUDCLNT_STREAMFLAGS_LOOPBACK` on the default **render**
  endpoint; mic = default **capture** endpoint. No cpal dependency.
- **Silence keep-alive render client.** WASAPI loopback delivers nothing (or `SILENT` buffers)
  when no app is playing, which would starve/short the audio track. A render client on the same
  endpoint is opened alongside and fed continuously — silence in normal use, and (via the
  `RUSTSHOT_TEST_TONE_HZ` env var) a sine tone in the functional test so loopback has a known
  signal to prove the path.
- **Canonical internal format 48 kHz stereo f32**, linear-resampled per source (mic may be
  44.1 kHz), mixed by summing with a soft clamp, then converted to **PCM16** for the sink — MF's
  AAC encoder rejects float input on some systems.
- **Sample-count audio clock, held to the video clock.** Audio sample time is
  `frames_written * 10^7 / 48000`, never wall time, so packets can't jitter the timeline. But
  the video is paced by the monotonic clock and the audio by the device's own crystal, and a
  stalled or unplugged device simply stops delivering, so the two *can* drift apart. The encoder
  loop therefore compares the samples written with the samples the active (unpaused) time calls
  for: more than 200 ms behind → silence is written to close the gap (leaving 50 ms so audio
  still in flight lands on time, not early); more than 200 ms ahead (a fast device clock) → the
  excess is trimmed from the next chunk. 200 ms sits above capture latency and scheduling jitter
  and below an obvious lip-sync slip; corrections are logged. Pre-roll captured during video
  startup is discarded at the loop start so both timelines begin at t = 0 (measured skew fell
  from ~0.43 s to ~0.04 s). Pause drops audio packets, matching the paused video.
- **Per-source alignment in the mixer.** When one source starves past the wait limit, the mixer
  emits with silence in its place and records the span as *owed*; that source's late audio for
  the span is dropped when it arrives, so a microphone hiccup doesn't leave the mic permanently
  behind the system audio. A source more than 500 ms ahead of the mixer has its oldest audio
  dropped. A source whose device errors (unplugged) becomes silence for the rest of the take.
- **Any shared-mode format.** The mix format is decoded from `nBlockAlign` and the sample
  container: PCM 8/16/24/32-bit (24-in-32 is MSB-aligned, so it reads as 32-bit) and float
  32/64; anything else is refused with a note instead of silently recording zeros. More than two
  channels are folded to stereo by `dwChannelMask` (centre and surrounds at -3 dB, LFE dropped;
  dropping the centre would lose the dialogue of a 5.1 mix). The resampler interpolates across
  buffer boundaries. `GetMixFormat` memory is freed (`CoTaskMemFree` guard).
- **Microphone choice.** `microphone_device` is matched against the active capture endpoints:
  the endpoint id, else the friendly name (case-insensitive), else the first name containing it.
  No match → the default microphone, with a note. Settings has a **Microphone** drop-down built
  from the same list and the same matching rule (`recorder::find_input`), so it shows what a
  recording will pick; it stores the device's *name* (readable in `config.toml`, survives the
  endpoint id changing when a USB device moves port). A configured device that is not connected
  is kept as a "not connected" entry rather than silently replaced on OK. The entry the
  configured value selects stores that value as written, so OK without touching the list never
  rewrites it (an id stays an id; "USB Mic" isn't turned into the "USB Mic 2" it currently
  matches). Devices that share a name are numbered in the list and stored by endpoint id.
- **Graceful degradation, reported.** Each source is opened independently: a missing or busy
  microphone no longer discards a working system-audio capture. Device setup overlaps the wait
  for the first video frame and may take up to 10 s (a Bluetooth headset switching profile);
  past that the audio thread is detached and the recording is video-only. A recording that
  stops before setup reported in (the video failed to start) waits 2 s, then detaches too, so
  stopping can't hang on a device stuck in activation. Every such case leaves a note on the
  recording, and the app shows them in one dialog after the file is saved ("…saved, but its
  sound was not recorded as set up"), not mid-recording, where a dialog would land in the video.
- **AAC muxed via the same `IMFSinkWriter`** (`AddStream` before `BeginWriting`), 48 kHz stereo,
  192 kbps (`AVG_BYTES_PER_SECOND = 24000`; the encoder only accepts 12k/16k/20k/24k).
- **Verification by signal, not ears** (`dev record-audio-test` + extended `probe-mp4`): decode
  the audio stream and assert RMS > 0.1 (present, not silence) and a zero-crossing dominant
  frequency near the rendered tone. Lip-sync by ear and live mic input remain manual QA.

## M5 — GIF

- **Reuse the WGC `CaptureStream`, encode with the `image` crate.** A separate `gif::GifSession`
  (mirroring the video `Session` API) captures frames, downscales each with
  `imaging::thumbnail` (longer edge → `gif_max_width`, never upscaling), and encodes a looping
  GIF via `image`'s `GifEncoder`. The `image` dependency gains the `gif` feature.
- **Timed by the same clock as video.** Frame slots are counted on the shared `FrameClock`; a
  frame still current at the next slot (static screen, or conversion slower than `gif_fps`) is
  stored once with a longer delay instead of being converted again, and delays are rounded to
  the GIF's centiseconds on the running total. Previously one frame was pushed per loop and the
  file assumed exactly `gif_fps`, so a 4K source at ~50 ms per conversion played ~1.7× fast.
  Measured after the change: 6.0 s recorded → 5.73 s of playback (the first frame arrives
  ~0.3 s in), 86 slots stored as 19 distinct frames.
- **Bounded in memory, by size too.** The frame-slot cap alone allows 600 s × 50 fps at up to
  7680 px, far more than fits in RAM, so the stored frames are also capped at 1 GiB; reaching it
  ends the recording with a notice, like the length cap. The encoder writes the GIF trailer in
  `Drop`, where the `gif` crate ignores errors, so the output goes through a writer that
  latches the first I/O error and the save fails (as "disk full" when it is) instead of
  renaming a truncated file into place.
- **Bounded in memory.** Capture stops at `gif_fps × gif_max_seconds` frame slots of
  unpaused time, and repeated frames cost nothing, so the in-RAM buffer is at most
  e.g. 15 fps × 30 s = 450 downscaled frames — no streaming GIF encoder needed. Reaching the cap
  ends the recording like any other early end, so the user gets a notice ("it reached the
  30-second limit for GIF recordings") instead of a toolbar that silently disappears.
- **One handle, two backends.** `RecorderHandle` wraps a `Backend::{Video, Gif}` enum so GIF and
  MP4 share pause/stop and the same recording flow (`flows::record`); the tray offers both.
- **Verified by re-decoding** (`imaging::probe_gif` with an independent `GifDecoder`): a live
  capture produced a 640×360, 14-frame animated GIF from a 1080p monitor.

## M6 — Polish

- **App crate layout** (review item 7). `crates/app` is a library with two thin binaries
  (`src/bin/rustshot.rs`, `src/bin/rustshot-dev.rs`). Inside: `ui/` (tray, overlay, flyout,
  annotate, recording toolbar, settings — the last split into `template`, `dialog` and
  `capture`), `flows/` (`capture`, `record`: the screenshot and recording sequences driven by
  the dispatch window), `platform/` (DPI scaling, owned GDI handles, `Rect`→`RECT`), and at the
  top level the dispatch window and app state, hotkeys, notify, logging, panic guard,
  single-instance, startup, clipboard and the `dev` commands. Each window keeps its own
  window procedure: they differ in who owns the state (the window, a struct, or a nested loop),
  so a generic wrapper would hide that; `panic_guard::guard` is the shared part.
- **One library, two binaries.** `crates/app` is a library (`rustshot_app`) exposing only
  `run_tray`, `run_dev` and `DevCmd`; everything else is crate-private. `rustshot.exe` is the
  GUI-subsystem tray app (`#![windows_subsystem = "windows"]`, no console), and `rustshot-dev.exe`
  is a plain console program for the diagnostics. This replaced a single GUI binary that
  attached to its parent console at startup so `rustshot dev …` could print — a hack that broke
  `2>file` redirects and needed care not to clobber pipes. Both binaries carry the same manifest
  (DPI awareness, themed controls). `rustshot.exe dev …` now shows a box pointing to
  `rustshot-dev.exe` instead of silently starting the tray. Only the tray exe ships in the zip.
- **Run-at-startup re-writes the path.** Enabling always writes `current_exe()` to the HKCU
  `Run` key (repairs a stale entry if the portable exe moved); disabling deletes it. Applied at
  startup and on settings OK.
- **Version block generated by build.rs.** The resource script is written to OUT_DIR so its
  VERSIONINFO carries the Cargo version (Explorer → Properties → Details); the tray app also logs
  its version at startup. Bug reports can then say which build they came from.
- **Manifest + icon via `embed-resource`/rc.exe**, embedding `app.manifest` (Per-Monitor-V2 DPI,
  `asInvoker`, Win10/11 supportedOS) and an `.ico` generated from the programmatic tray icon.
  `SetProcessDpiAwarenessContext` is kept as well so the un-manifested dev/test binaries are
  still DPI-correct. Resource embedding is optional for debug builds so a toolchain without
  rc.exe still builds, but a **release** build fails without it (it would otherwise ship
  DPI-unaware and iconless). The manifest also requests **Common Controls v6**, so buttons,
  checkboxes and edits are themed (Windows 11 look) instead of the Windows 2000 fallback;
  verified with a capture of the live Settings dialog and `dev settings-selftest`.
- **Hand-placed windows scale with the monitor's DPI** (`app/src/platform/dpi.rs`). Per-Monitor-V2
  means Windows no longer stretches our windows, so the flyout, recording toolbar and editor
  toolbar, whose layouts are written in 96-DPI pixels, are scaled by the DPI of the monitor they
  open on (verified at 125 %: flyout 625×212, toolbar 300×70) and use the system message font at
  that DPI (before, the flyout and toolbar buttons had no font set at all). The flyout and
  toolbar open on the monitor under the pointer, inside its work area (clear of the taskbar),
  rather than at fixed offsets from the primary screen's corner. Only the editor handles
  `WM_DPICHANGED` (it is the one long-lived, draggable window); the flyout lives for seconds and
  the toolbar stays put, so both keep their creation DPI. The 32-px crosshair cursor is not
  scaled. Template dialogs (Settings) are scaled by the dialog manager.
- **Window state is owned, not aliased.** The editor's state reaches its window through a
  thread-local that `WM_NCCREATE` adopts, so it is freed exactly once however
  `CreateWindowExW` fails; no `&mut` to window state is held across a call that pumps messages
  (`MessageBoxW`, `DestroyWindow`); GDI handles live in guards so error paths don't leak them.
  The Settings folder picker is owned by the dialog (modal to it).
- **Files are written atomically and never clobbered.** Screenshots and annotated copies claim
  their name with an exclusive create, then write to a temporary sibling and rename over the
  placeholder; the rename is retried briefly while a sync client (OneDrive) or scanner holds the
  new file open. The daily logs keep 14 files; `panic.log` rolls over at 1 MB.
- **Shortcut names are validated against what can be registered.** `config` holds the list of
  key names (W3C spelling, with aliases such as `Up`, `Caps`, `Menu`); anything else fails to
  parse, so Settings refuses it on OK instead of saving a key that silently never fires. A test
  keeps that list and the registration mapping in agreement.
- **Settings dialog from an in-memory `DLGTEMPLATE`, not egui.** eframe wants the main-thread
  event loop, which already belongs to the tray pump + nested overlay/dialog loops — three event
  loops would fight. A classic dialog template gives tab order, the default button, and
  Enter/Esc for free via `DialogBoxIndirectParamW`. The template builder (`DlgBuilder`) is pure
  and unit-tested (the testable core for M6); the dialog itself is visual/manual-QA.
- **Apply-on-OK re-registers hotkeys**, so the `Hotkeys` handle moved from `start()`'s stack into
  `AppState`. Re-registration drops the old manager *first* (unregistering its keys) before
  creating the new one, avoiding a self-conflict on unchanged bindings.
- **Folder picker via `rfd`** (already a workspace dep; wants STA, which the UI thread is).
- **User docs in two layers.** `USER-GUIDE.md` is the compact reference (every `config.toml`
  key, every shortcut key name) and ships in the zip; `MANUAL-GUIDE.md` is the task-by-task
  walkthrough, with a Vietnamese translation, `MANUAL-GUIDE.vi.md`, for the first users. Both
  keep RustShot's on-screen labels in English, exactly as the app shows them, so a reader can
  find each button. A UI change updates all three files.

## After 0.1.8 — annotation tools

- **Two layers: pixels, then vectors.** Highlight and pixelate change the screenshot's pixels
  (`model::apply_pixel_layer`, in the order they were drawn); rectangles, arrows and text are
  drawn with GDI on top (`win::paint_items`). Save and Copy both go through `render`, which
  encodes the result, so a redaction is in the exported pixels, not an overlay that a viewer or
  editor could lift off. Consequences, accepted: a pixelate never blurs your own arrow or text
  (they are always above), and a highlight never tints them. The canvas shows the same pixel
  layer: the source surface is rebuilt from `apply_pixel_layer` whenever the set of pixel items
  changes (draw, undo, redo), which costs one image copy per change, not per paint.
- **Pixelate = block mean, 12-px blocks, pure function in `imaging`.** Each square becomes its
  rounded mean, so every input with the same block means gives the same output (a many-to-one
  map: there is nothing to invert). Blocks under 2 px are raised to 2 (1 px would be a no-op
  passing for a redaction), and a leftover strip at the right/bottom edge narrower than half a
  block joins the previous block instead of being averaged on its own, where a 1-px sliver would
  keep a column of detail. Known limit, documented for users: pixelation of *large* text or a
  well-known logo can stay recognisable by shape, and dictionary attacks on pixelated text exist
  when blocks are small relative to the glyphs; 12 px is larger than the stroke height of normal
  UI text at 100–150 %. A solid fill would be stronger; the request was pixelate, so that is a
  possible later tool. The original screenshot is untouched (as for every annotation); the
  guides say to share the `_annotated` copy.
- **Highlight = alpha blend at 40 %, not multiply.** Multiply is the "real highlighter" look but
  makes black a solid fill and white a no-op; a plain blend keeps every palette colour
  translucent. The drag preview uses GDI `AlphaBlend` with the same constant alpha; after
  release the blended pixels come from `imaging::blend_rect`, the same as the export.
- **Arrow geometry is pure** (`model::arrow_geometry`): head 4.5 × line width, at least 14 px,
  at most half the arrow; barbs ±~27°; the shaft stops at the base of the filled head so a thick
  line's cap never pokes past the point. `MIN_SIZE` for an arrow is a length, since a level arrow
  has zero height.
- **Fixed palette and three widths; per-tool style.** Six colours (red, yellow, green, blue,
  black, white) and thin/medium/thick = 2/3/6 px. Medium is the editor's old 3 px and red the
  old colour, so a user who never touches the new controls gets exactly the 0.1.8 output. Each
  tool remembers its own style (the highlighter starts yellow); every item stores the style it
  was drawn with. Choosing a colour while typing restyles the open text box.
- **Redo is a stack cleared by any new item** (including committing text), and it waits while a
  shape or text box is in progress; undo of an in-progress shape only cancels it and leaves the
  redo stack alone.
- **Unsaved = items differ from the last save** (`Ctx::dirty`), not a flag kept up to date by
  hand. The flag missed one path: clicking away from a text box commits it, and that never set
  it, so closing the editor dropped the text without asking. An editor emptied by undo has
  nothing to save and does not ask.
- **One-key shortcuts come from `WM_CHAR`, not key codes** (`model::shortcut`): `R T A H P`,
  `1`–`6`, `[ ]` then work on any layout (JIS included), and they are ignored while typing so
  they cannot steal text. An IME in composition mode receives them instead; the guides say so.
- **Owner-drawn swatches and line samples.** A text label cannot show a colour; the buttons keep
  names as window text for screen readers. Updating them (`EnableWindow`, `CheckRadioButton`)
  can repaint synchronously and re-enter the window procedure with `WM_DRAWITEM`, so
  `sync_toolbar` finishes with the state before touching the buttons and is the last use of the
  state in each handler.
- **Verification boundary.** Written on Linux: `imaging` and the editor model (run through a
  scratch crate that includes `model.rs`, since the app crate does not build there) are tested;
  the window code is type-checked and linted for `x86_64-pc-windows-msvc` only. The QA checklist
  has the manual steps.

## 0.1.10 — black box, select, step markers

- **Black box is a pixel item** (`imaging::blend_rect` at full opacity), like pixelate: the
  export holds black pixels, not a black shape over the original. It exists because pixelation
  keeps the rough shape of large text and logos. It has no colour on purpose — a coloured fill
  is not what anyone means by redaction, and a white one would look like empty space.
- **Undo became a history of snapshots** of the item list (one per change) instead of "pop the
  last item". Moves, deletes and restyles are not additions, so a pop-based undo could not take
  them back. The list is small (dozens of items), so a snapshot per change costs nothing that
  matters. Every earlier undo/redo test passes unchanged, which is the evidence that drawing
  behaves as before.
- **Hit testing is pure** (`Item::hit`, `Editor::hit_test`): vector items are checked before
  pixel items (they are drawn above them), each group from the top down; a rectangle outline is
  hit only near its edge so items inside it stay clickable; text boxes and filled areas anywhere
  inside; arrows by distance to the shaft or the head. The slack is 5 screen px converted to
  image px by the window, so it feels the same at any zoom.
- **Moves are clamped by an anchor**: an item's bounds must stay inside the image, except a step
  marker, whose centre must (the same rule as when it is placed). The move is recomputed from the
  item as it was at the start of the drag each time, so clamping at an edge never drifts.
- **The selected item restyles from the toolbar**, and the toolbar shows its style; the tools'
  own styles are untouched, so switching back to a drawing tool shows that tool's colour.
- **A dragged pixel item is left out of the canvas** and drawn as its preview until release, so
  dragging a redaction rebuilds the full-resolution canvas twice, not on every mouse move. The
  cost is that the content under a pixelate area or black box shows on screen during the drag;
  the export never includes it (it renders from the items, not from the canvas).
- **Step numbers = highest + 1**, never renumbered automatically: renumbering on delete would
  silently change markers the user already referred to in their text. Undo of the last marker
  frees its number. Numbers are drawn in the palette colour's contrast colour (white on dark,
  black on yellow and white) and shrink for two or three digits. The line thickness picks the
  diameter (22 / 30 / 44 px) rather than adding a separate size control.
- **Toolbar layout is computed** from each button's gap and width (`button_x`, `TOOLBAR_W`), so
  adding a button cannot leave stale x positions; a test checks that no buttons overlap and that
  the toolbar fits the minimum width (now about 1010 px at 96 DPI).

## Crop, Pin to screen, History

- **Crop is part of the document, not a pixel operation.** `model::Doc` = items + crop; undo
  snapshots and the "unsaved work" check use both, so a crop can be undone, redone, replaced or
  removed like a mark, and coordinates stay in the full image (annotations are never shifted).
  Save and Copy render at full size and cut last (`win::export`), so redactions and outlines at
  the edge are cut exactly where the crop says. Both corner pixels are kept (inclusive), so a
  corner-to-corner drag keeps the whole image. A click with the Crop tool removes the crop: the
  cheapest way to undo-without-history, and undo still brings it back.
- **A pin shows device pixels 1:1** at 100 %: a screenshot's pixels are device pixels, so a pinned
  region covers exactly what it captured. It ignores `WM_DPICHANGED` for the same reason. It opens
  at the largest zoom step that fits 80 % of the work area. The window is a top-most tool window
  (no taskbar button) whose whole client area is a caption (`HTCAPTION`), so dragging is native.
  It holds its own DIB of the image (not the `Rgba` too), so several pins of 4K captures stay
  affordable, and deleting the file never breaks one. The right-click menu runs `TrackPopupMenu`
  with `TPM_RETURNCMD` and no state reference held across it, like the editor's message boxes.
- **History lives in `config`** (pure, TOML, atomic write shared with `Settings`), so it is tested
  on any platform, and in its own file (`history.toml`) rather than `config.toml`, which users
  edit by hand. Entries store a preformatted local time instead of a timestamp, so the app needs
  no new time dependency to show it. Recording is best effort (logged, never a dialog): the
  capture already succeeded. Missing files are dropped when the window opens, and the file is
  rewritten only if something was dropped. Path matching is exact (the app always records the
  same spelling it saved). `history.max_entries` is bounded 10–1000; `enabled = false` stops
  recording without deleting the existing list.
- **History window = plain LISTBOX + painted preview**, not a ListView: one column of text is
  enough, and it needs no common-controls initialisation. Buttons act on a path copied out of the
  state first, because the editor, a pin, Explorer and the confirmation boxes can all pump
  messages back into the window procedure. "Delete file…" is permanent (no Recycle Bin, which
  would need `IFileOperation`) and says so in its question.

## Save feedback, resize handles, Ellipse / Line / Spotlight

- **Save answers with the flyout, not by changing the canvas.** The report was "I cropped,
  pressed Save, nothing changed": the editor keeps the full picture with the crop only shaded,
  because the crop is part of the document and must stay editable. Rather than close the editor
  or swap its picture for the saved one (both lose the ability to go on), Save now shows the
  post-capture flyout for the `_annotated` file, with the exported image — cropped and annotated
  — as the thumbnail. It is the same answer a capture gives, and puts Open / Folder / Copy / Pin
  next to the result. The title-bar note stays. Known and accepted: the flyout's Delete removes
  the file the editor considers saved (closing then does not ask again), and its Annotate opens
  the copy in a second editor.
- **`save` returns what happened; the handler reports it last.** Creating the flyout and showing
  a message box can both re-enter the editor's window procedure, so neither may run while the
  window state is in use. `save` only exports, writes and sets the title, and hands back a
  `SaveOutcome`; `report_save` (history entry, flyout, or `notify::error` with the reason) runs
  after `sync_toolbar`, as the last thing in the handler, and takes no state. The app state is
  borrowed twice, briefly: to read the dispatch window, and to store the new flyout. The flyout
  it replaces is dropped outside the borrow, since dropping destroys a window. An editor running
  without the tray app (no app state) simply shows no flyout.
- **A resize is a move with a handle.** `Moving` gained `handle: Option<(Handle, Point)>`; all
  the rest — recomputing from the item as it was at mouse-down, one undo step on release, Esc
  and undo cancelling, the pixel item left out of the canvas — is the existing move path, so it
  cannot behave differently. The handle goes to *its original place plus the pointer's travel*,
  not to the pointer: a handle is grabbed up to 5 px off its centre, and a plain click on it must
  not nudge the edge.
- **Resize math is per axis and has a dead zone instead of a floor.** A box handle moves the left
  or right edge and/or the top or bottom edge; the others stay. The result is normalized, so
  crossing the opposite edge flips the box. Within `MIN_SIZE` of the opposite edge the moved edge
  waits `MIN_SIZE` away *on the side it started from*: that spot is always inside the picture
  (the item was at least that large there), so no second clamp can fight the first, and the box
  flips only once the pointer is a full `MIN_SIZE` past. An end of an arrow or line too close to
  the other end is pushed out to `MIN_SIZE` along the pointer's direction (rounded away, so
  never shorter), or stays where it was if that point is outside the picture. Ends are never
  swapped: an arrow's head stays on its end.
- **Handles act only on the item that is already selected, and are tested before anything else.**
  A first click on a corner of an unselected box therefore still moves it, as before (and the
  existing move tests pass unchanged); once selected, its handles win over other items under the
  pointer. The slack is the hit-test slack on each axis, since handles are drawn as squares.
- **Midpoint handles disappear on short sides** (under four times the slack): on a small box they
  would cover the whole edge and leave nowhere to grab the box to move it. Below twice the slack
  even the corners cover everything; such a box can be enlarged first. Step markers get no
  handles because their size is the line setting, which the toolbar already changes.
- **The cursor over an arrow's end follows the arrow** (the nearest of the four resize cursors):
  an end has no "corner" to name.
- **Ellipse is hit on its outline; so is Spotlight.** Both frame something else. An ellipse uses
  the same rule as the hollow rectangle with ellipses instead of rectangles (inside the outer,
  outside the inner). A spotlight's inside is the untouched screenshot, so a click there should
  reach what it frames; picking it anywhere inside would make a large spotlight swallow every
  click on a pixel item under it.
- **Line is its own item, not an arrow with a flag.** It shares the length rule, the hit test of
  the shaft and the two end handles through small helpers (`Item::ends`), but an arrow item with
  a "no head" field would have changed the shape of every existing arrow value and test.
- **Spotlights darken once, outside their union, and last.** `imaging::dim_outside(img, keep,
  alpha)` blends black over every pixel that is in none of the rectangles (per row: mark the
  kept spans, darken the rest), so two spotlights can never stack into a darker background and
  the result does not depend on the order they were drawn in. In `apply_pixel_layer` this is the
  one exception to "in order": it runs after the other pixel items, so a highlight or redaction
  outside the spotlights is dimmed with the background whether it was drawn before or after.
  Applying it at the first spotlight's position was the alternative; then the same picture
  would look different depending on drawing order, with no way to see why. Vector items are
  drawn afterwards and stay at full brightness, which is what an arrow pointing into the
  spotlight needs. 55 % black: darker than a highlight is light, lighter than the crop shade, so
  the background still reads as context.
- **A spotlight keeps both corner pixels**, like a crop: boxes in the editor are spans between
  two clamped pixel positions, so without this a spotlight dragged into the corner of the
  picture would leave a one-pixel dark line along two edges.
- **Live spotlights are drawn with a clip region, because darkening cannot be painted over.**
  For the other pixel items the canvas keeps the committed ones baked in and paints the dragged
  one on top. A second spotlight would have to *brighten* part of an already darkened canvas.
  So while any spotlight is drawn, moved or resized, all spotlights are left out of the canvas
  source (`Editor::canvas_pixels`, now the one place that decides this) and the window darkens
  the picture with a single `AlphaBlend` whose clip region excludes every kept area
  (`Editor::live_spotlights`) — the union again, with one canvas rebuild at each end of the drag.
- **The toolbar is two rows.** One row was 1058 px at 96 DPI; with Ellipse, Line and Spotlight it
  would be about 1240 px, wider than a 1366-px screen at 125 % (1092 px) — the window is never
  narrower than its toolbar. Shortening labels and buttons to fit would have needed "Rect",
  "Hilite", "Pixel" and 22-px swatches and left no room for the next tool. The tools now have
  the first row and colours, line widths, Undo, Redo, Save and Copy the second: 722 px wide, 74
  px tall (34 px less canvas), every label as before. `button_x` sums along a button's row,
  `button_y` places the row, and a compile-time assertion fails the build if the toolbar grows
  past 1060 px. Undo / Redo / Save / Copy moved to ids 240–243 so the consecutive tool ids have
  room.
