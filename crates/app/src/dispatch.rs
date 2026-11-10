//! The action bus. Menu clicks, global hotkeys, the delay timer, and flyout buttons all run in
//! `Fn + Send + Sync + 'static` callbacks that cannot touch application state directly, so they
//! instead `PostMessage` an [`Action`] to a hidden message-only window. Its window procedure
//! runs on the UI thread and owns all the state via a thread-local [`AppState`].

use std::cell::RefCell;
use std::path::PathBuf;

use anyhow::{Context, Result};
use config::Settings;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, PostMessageW, PostQuitMessage, RegisterClassW, CW_USEDEFAULT,
    HMENU, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WNDCLASSW,
};

use crate::flows;
use crate::ui::flyout;
use crate::ui::flyout::Flyout;

/// Custom message carrying an [`Action`] in its `wParam`.
pub const WM_RUSTSHOT_ACTION: u32 = WM_APP + 1;

/// One user-facing action. `Copy` + `repr(u32)` so it round-trips through a `WPARAM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Action {
    ShotRegion = 1,
    ShotWindow,
    ShotMonitor,
    ShotAllMonitors,
    RecordRegion,
    StopRecording,
    ExecuteDelayedShot,
    OpenFolder,
    Settings,
    Quit,
    FlyoutOpen,
    FlyoutOpenFolder,
    FlyoutCopy,
    FlyoutCopyPath,
    FlyoutDelete,
    FlyoutDismiss,
    ExecuteRecordStart,
    RecordPauseToggle,
    RecordGif,
    FlyoutAnnotate,
    /// The recording toolbar's tick: check whether the recording ended on its own.
    RecordPoll,
    FlyoutPin,
    /// Tray → History…: open (or raise) the History window.
    History,
}

impl Action {
    fn from_u32(v: u32) -> Option<Action> {
        use Action::*;
        Some(match v {
            1 => ShotRegion,
            2 => ShotWindow,
            3 => ShotMonitor,
            4 => ShotAllMonitors,
            5 => RecordRegion,
            6 => StopRecording,
            7 => ExecuteDelayedShot,
            8 => OpenFolder,
            9 => Settings,
            10 => Quit,
            11 => FlyoutOpen,
            12 => FlyoutOpenFolder,
            13 => FlyoutCopy,
            14 => FlyoutCopyPath,
            15 => FlyoutDelete,
            16 => FlyoutDismiss,
            17 => ExecuteRecordStart,
            18 => RecordPauseToggle,
            19 => RecordGif,
            20 => FlyoutAnnotate,
            21 => RecordPoll,
            22 => FlyoutPin,
            23 => History,
            _ => return None,
        })
    }
}

/// Post an action to the dispatch window from any thread/callback.
pub fn post(hwnd: HWND, action: Action) {
    // SAFETY: PostMessageW is thread-safe; the window outlives all senders (it is destroyed only
    // on Quit, after which no more actions are posted).
    unsafe {
        let _ = PostMessageW(
            Some(hwnd),
            WM_RUSTSHOT_ACTION,
            WPARAM(action as u32 as usize),
            LPARAM(0),
        );
    }
}

/// Post a timer-driven action (`ExecuteDelayedShot`, `ExecuteRecordStart`) together with the
/// [`TimerEpoch`] token its timer was armed with; the dispatch window drops it if the timer has
/// been cancelled or superseded since.
pub fn post_timed(hwnd: HWND, action: Action, token: u32) {
    // SAFETY: as for `post`.
    unsafe {
        let _ = PostMessageW(
            Some(hwnd),
            WM_RUSTSHOT_ACTION,
            WPARAM(action as u32 as usize),
            LPARAM(token as isize),
        );
    }
}

/// Identifies the one delay/countdown timer that may still fire. The timers are plain sleeping
/// threads that cannot be stopped, so each carries the token it was armed with and the dispatch
/// window ignores any whose token is no longer current. Without this, a countdown cancelled
/// with Stop would still fire later and act on whatever flow was running by then: releasing its
/// `busy` gate, or starting its recording before its own countdown ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimerEpoch(u32);

impl TimerEpoch {
    /// Start a new timer: every earlier token becomes stale. Returns the new timer's token.
    pub fn arm(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(1);
        self.0
    }

    /// Cancel the armed timer, if any: its token becomes stale.
    pub fn cancel(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }

    /// True if `token` belongs to the timer armed last and not cancelled since.
    pub fn is_current(self, token: u32) -> bool {
        self.0 == token
    }
}

/// All UI-thread-owned application state.
pub struct AppState {
    pub settings: Settings,
    pub output_folder: PathBuf,
    pub dispatch_hwnd: HWND,
    /// True while a capture/overlay is in progress; a second trigger is dropped.
    pub busy: bool,
    /// The most recent post-capture flyout, kept alive until dismissed/replaced.
    pub flyout: Option<Flyout>,
    /// The shot kind queued behind the delay timer.
    pub pending_shot: Option<flows::capture::ShotKind>,
    /// The in-progress recording (handle + toolbar), if any.
    pub recording: Option<crate::ui::recording::Recording>,
    /// A recording request queued behind the countdown.
    pub pending_record: Option<recorder::RecordCfg>,
    /// Which delay/countdown timer is still allowed to fire (see [`TimerEpoch`]).
    pub timer_epoch: TimerEpoch,
    /// Registered global hotkeys (kept alive; replaced when settings change them).
    pub hotkeys: Option<crate::hotkeys::Hotkeys>,
    /// The tray icon (removed from the notification area when dropped).
    pub tray: Option<tray_icon::TrayIcon>,
}

/// Set the tray tooltip from the current hotkey state ("N hotkeys unavailable", or idle).
/// Called at startup and whenever the hotkeys are re-registered.
pub fn refresh_tray_tooltip() {
    // Copy out under the borrow, call the shell outside it (the rule for every shell call).
    let Some((Some(tray), tip)) = with_app(|a| {
        let tip = a
            .hotkeys
            .as_ref()
            .map_or_else(|| "RustShot".to_string(), |h| h.tooltip());
        (a.tray.clone(), tip)
    }) else {
        return;
    };
    let _ = tray.set_tooltip(Some(tip));
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

/// Run `f` with mutable access to the app state, if initialized.
pub fn with_app<R>(f: impl FnOnce(&mut AppState) -> R) -> Option<R> {
    APP.with(|cell| cell.borrow_mut().as_mut().map(f))
}

/// Remove and return the app state. Called once after the message loop exits so that
/// everything it owns (an active recording — which finalizes the file — the flyout, the
/// hotkeys) is dropped deterministically on the UI thread, whatever posted the `WM_QUIT`.
/// Left to process exit, the recording's encoder thread could never be joined.
pub fn take_state() -> Option<AppState> {
    APP.with(|cell| cell.borrow_mut().take())
}

/// Create the hidden dispatch window and install `state` (which it takes ownership of). Returns
/// the window handle used as the action target.
pub fn create(settings: Settings, output_folder: PathBuf) -> Result<HWND> {
    const CLASS: PCWSTR = w!("RustShotDispatch");
    // SAFETY: standard window-class registration + message-only window creation.
    let hwnd = unsafe {
        let hinstance = GetModuleHandleW(None).context("GetModuleHandleW")?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        // Ignore a duplicate-class error (harmless if already registered).
        let _ = RegisterClassW(&wc);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("RustShot"),
            WINDOW_STYLE(0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(HWND_MESSAGE),
            Some(HMENU(std::ptr::null_mut())),
            Some(hinstance.into()),
            None,
        )
        .context("creating dispatch window")?
    };

    APP.with(|cell| {
        *cell.borrow_mut() = Some(AppState {
            settings,
            output_folder,
            dispatch_hwnd: hwnd,
            busy: false,
            flyout: None,
            pending_shot: None,
            recording: None,
            pending_record: None,
            timer_epoch: TimerEpoch::default(),
            hotkeys: None,
            tray: None,
        });
    });

    Ok(hwnd)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A panic must not unwind across this FFI boundary (that aborts the process): run the real
    // handler under the panic guard and fall back to default processing.
    crate::panic_guard::guard(
        "dispatch wndproc",
        || unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        || unsafe { wndproc_inner(hwnd, msg, wparam, lparam) },
    )
}

unsafe fn wndproc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_RUSTSHOT_ACTION {
        if let Some(action) = Action::from_u32(wparam.0 as u32) {
            // A timer that was cancelled (or replaced by a newer one) still posts when its
            // sleep ends; its token no longer matches, and it must not touch the current flow.
            let stale = matches!(
                action,
                Action::ExecuteDelayedShot | Action::ExecuteRecordStart
            ) && !with_app(|app| app.timer_epoch.is_current(lparam.0 as u32))
                .unwrap_or(false);
            if stale {
                tracing::debug!("{action:?} from a cancelled timer; ignored");
            } else {
                handle(action);
            }
        }
        return LRESULT(0);
    }
    // Let the flyout's child-button notifications flow through if routed here in future.
    // SAFETY: standard default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Route an [`Action`] to its handler on the UI thread.
fn handle(action: Action) {
    use Action::*;
    match action {
        ShotRegion => flows::capture::begin(flows::capture::ShotKind::Region),
        ShotWindow => flows::capture::begin(flows::capture::ShotKind::Window),
        ShotMonitor => flows::capture::begin(flows::capture::ShotKind::Monitor),
        ShotAllMonitors => flows::capture::begin(flows::capture::ShotKind::AllMonitors),
        ExecuteDelayedShot => flows::capture::execute_pending(),
        RecordRegion => flows::record::begin(false),
        RecordGif => flows::record::begin(true),
        ExecuteRecordStart => flows::record::execute_start(),
        RecordPauseToggle => flows::record::toggle_pause(),
        StopRecording => flows::record::stop(),
        RecordPoll => flows::record::poll(),
        OpenFolder => {
            // Clone the path out, then shell out *outside* the borrow (explorer can pump).
            if let Some(folder) = with_app(|app| app.output_folder.clone()) {
                flyout::open_in_explorer(&folder);
            }
        }
        Settings => crate::ui::settings::open(),
        History => crate::ui::history::open(),
        Quit => {
            tracing::info!("Quit selected");
            // Finalize an active recording (or cancel a countdown) and drop the flyout *before*
            // leaving the loop, so the output file is playable and no window outlives the state.
            flows::record::stop();
            with_app(|app| {
                app.flyout = None;
                app.pending_shot = None;
                app.timer_epoch.cancel();
            });
            // SAFETY: posts WM_QUIT to this (UI) thread's queue.
            unsafe { PostQuitMessage(0) };
        }
        // For every flyout action we copy the path out under a short borrow, release it, and
        // only then run shell/clipboard work — which can pump messages and re-enter `handle`,
        // and must not find the `RefCell` still borrowed (that would panic in a wndproc).
        FlyoutOpen => {
            if let Some(p) = flyout_path() {
                flyout::shell_open(&p);
            }
        }
        FlyoutOpenFolder => {
            if let Some(p) = flyout_path() {
                flyout::select_in_explorer(&p);
            }
        }
        FlyoutCopy => {
            if let Some(p) = flyout_path() {
                match imaging::load_rgba(&p) {
                    Ok(img) => {
                        if let Err(e) = crate::clipboard::copy_image(&img) {
                            crate::notify::warn(
                                "The image could not be copied to the clipboard.",
                                e,
                            );
                        }
                    }
                    Err(e) => {
                        crate::notify::warn("The screenshot could not be reopened for copying.", e)
                    }
                }
            }
        }
        FlyoutCopyPath => {
            if let Some(p) = flyout_path() {
                if let Err(e) = crate::clipboard::copy_text(&p.to_string_lossy()) {
                    crate::notify::warn("The path could not be copied to the clipboard.", e);
                }
            }
        }
        FlyoutDelete => {
            // Take the flyout out (dropping it destroys the window), then delete the file.
            if let Some(f) = with_app(|app| app.flyout.take()).flatten() {
                let path = f.path().to_owned();
                drop(f);
                match std::fs::remove_file(&path) {
                    Ok(()) => tracing::info!("deleted {}", path.display()),
                    Err(e) => {
                        crate::notify::warn(&format!("Could not delete {}", path.display()), e)
                    }
                }
            }
        }
        FlyoutAnnotate => {
            // Close the flyout (the editor takes over), then open the editor *outside* the
            // borrow: creating a window can pump messages and re-enter `handle`.
            if let Some(f) = with_app(|app| app.flyout.take()).flatten() {
                let path = f.path().to_owned();
                drop(f);
                crate::ui::annotate::open(&path);
            }
        }
        FlyoutPin => {
            // Like Annotate: the pinned window takes over from the flyout, and is created
            // outside the borrow.
            if let Some(f) = with_app(|app| app.flyout.take()).flatten() {
                let path = f.path().to_owned();
                drop(f);
                crate::ui::pin::open(&path);
            }
        }
        FlyoutDismiss => {
            with_app(|app| {
                app.flyout = None;
            });
        }
    }
}

/// The current flyout's saved path, copied out under a short borrow (released before use).
fn flyout_path() -> Option<PathBuf> {
    with_app(|app| app.flyout.as_ref().map(|f| f.path().to_owned())).flatten()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn every_action_round_trips_through_its_u32() {
        use Action::*;
        // Keep in sync with the enum: a variant missing here (or in `from_u32`) would be
        // silently dropped by the dispatch window at runtime.
        let all = [
            ShotRegion,
            ShotWindow,
            ShotMonitor,
            ShotAllMonitors,
            RecordRegion,
            StopRecording,
            ExecuteDelayedShot,
            OpenFolder,
            Settings,
            Quit,
            FlyoutOpen,
            FlyoutOpenFolder,
            FlyoutCopy,
            FlyoutCopyPath,
            FlyoutDelete,
            FlyoutDismiss,
            ExecuteRecordStart,
            RecordPauseToggle,
            RecordGif,
            FlyoutAnnotate,
            RecordPoll,
            FlyoutPin,
            History,
        ];
        for a in all {
            assert_eq!(Action::from_u32(a as u32), Some(a), "{a:?}");
        }
        assert_eq!(Action::from_u32(0), None);
        assert_eq!(Action::from_u32(all.len() as u32 + 1), None);
    }

    #[test]
    fn only_the_timer_armed_last_is_current() {
        let mut epoch = TimerEpoch::default();
        let first = epoch.arm();
        assert!(epoch.is_current(first));
        // A newer timer supersedes the older one.
        let second = epoch.arm();
        assert!(epoch.is_current(second));
        assert!(!epoch.is_current(first));
    }

    #[test]
    fn a_cancelled_timer_stays_stale_after_the_next_one_is_armed() {
        // Regression: Stop during a countdown, then a new recording. The first countdown's
        // thread still posts; it must neither be honoured nor mistaken for the new timer.
        let mut epoch = TimerEpoch::default();
        let cancelled = epoch.arm();
        epoch.cancel();
        assert!(!epoch.is_current(cancelled));
        let next = epoch.arm();
        assert!(epoch.is_current(next));
        assert!(!epoch.is_current(cancelled));
        // A plain `post` (lParam 0) is never a valid token once any timer was armed.
        assert!(!epoch.is_current(0));
    }

    #[test]
    fn timer_tokens_wrap_without_panicking() {
        let mut epoch = TimerEpoch(u32::MAX);
        let before = u32::MAX;
        let token = epoch.arm();
        assert_eq!(token, 0);
        assert!(epoch.is_current(token));
        assert!(!epoch.is_current(before));
    }
}
