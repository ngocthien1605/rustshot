//! The History window (tray → History…): the captures RustShot saved, newest first, with a
//! preview of the selected one and buttons to open, annotate, pin, copy, show, forget or delete
//! it. One window at a time; it refreshes when a new capture is recorded. The list itself is
//! `config::history` (pure, unit-tested); this is the Win32 window around it.
//!
//! Every button acts on a path copied out of the window state first: opening the editor, a
//! pinned window, Explorer or a message box can pump messages and re-enter this procedure, so no
//! reference to the state is held across those calls.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::Path;
use std::ptr::null_mut;
use std::sync::Once;

use anyhow::{Context, Result};
use config::history::Entry;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetSysColorBrush,
    InvalidateRect, SelectObject, SetBkMode, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER,
    COLOR_BTNFACE, DIB_RGB_COLORS, DT_EDITCONTROL, DT_NOPREFIX, DT_WORDBREAK, HGDIOBJ, PAINTSTRUCT,
    SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetDlgItem, GetWindowLongPtrW, MessageBoxW, RegisterClassW,
    SendMessageW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, GWLP_USERDATA, HMENU, IDYES,
    LBN_DBLCLK, LBN_SELCHANGE, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY, LB_ADDSTRING, LB_GETCURSEL,
    LB_RESETCONTENT, LB_SETCURSEL, MB_ICONWARNING, MB_YESNO, SW_RESTORE, SW_SHOW, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_ACTIVATE, WM_COMMAND, WM_ERASEBKGND, WM_NCCREATE, WM_NCDESTROY, WM_PAINT,
    WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_MINIMIZEBOX, WS_OVERLAPPED,
    WS_SYSMENU, WS_VISIBLE, WS_VSCROLL,
};

const CLASS_NAME: PCWSTR = w!("RustShotHistory");
static REGISTER: Once = Once::new();

/// Layout at 96 DPI (scaled by the DPI of the monitor it opens on).
const CLIENT_W: i32 = 780;
const CLIENT_H: i32 = 470;
const LIST: (i32, i32, i32, i32) = (8, 8, 440, 414);
const PREVIEW: (i32, i32, i32, i32) = (456, 8, 316, 260);
const INFO: (i32, i32, i32, i32) = (456, 276, 316, 140);
const BUTTON_Y: i32 = 432;
const BUTTON_H: i32 = 28;

const ID_LIST: i32 = 300;
const ID_OPEN: i32 = 301;
const ID_ANNOTATE: i32 = 302;
const ID_PIN: i32 = 303;
const ID_COPY: i32 = 304;
const ID_FOLDER: i32 = 305;
const ID_REMOVE: i32 = 306;
const ID_DELETE: i32 = 307;
const ID_CLEAR: i32 = 308;

/// Buttons: id, label, width at 96 DPI (left to right, 6 px apart).
const BUTTONS: [(i32, PCWSTR, i32); 8] = [
    (ID_OPEN, w!("Open"), 60),
    (ID_ANNOTATE, w!("Annotate"), 72),
    (ID_PIN, w!("Pin"), 48),
    (ID_COPY, w!("Copy"), 52),
    (ID_FOLDER, w!("Folder"), 60),
    (ID_REMOVE, w!("Remove"), 64),
    (ID_DELETE, w!("Delete file…"), 84),
    (ID_CLEAR, w!("Clear list…"), 80),
];
const BUTTON_GAP: i32 = 6;

/// A preview thumbnail in top-down BGRA, for `StretchDIBits`.
struct Preview {
    bgra: Vec<u8>,
    width: i32,
    height: i32,
}

/// Per-window state, owned by the window (freed in `WM_NCDESTROY`).
struct Ctx {
    /// The entries shown, in list order.
    entries: Vec<Entry>,
    preview: Option<Preview>,
    /// The selected file's path and details, under the preview.
    info: String,
    dpi: u32,
    font: Option<crate::platform::dpi::UiFont>,
}

thread_local! {
    static PENDING: RefCell<Option<Box<Ctx>>> = const { RefCell::new(None) };
    /// The open History window, if any (there is at most one).
    static OPEN: Cell<isize> = const { Cell::new(0) };
}

fn open_hwnd() -> Option<HWND> {
    let raw = OPEN.with(Cell::get);
    (raw != 0).then_some(HWND(raw as *mut c_void))
}

/// Open the History window, or bring the open one to the front.
pub fn open() {
    if let Some(hwnd) = open_hwnd() {
        // SAFETY: restore and raise our own window.
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    if let Err(e) = try_open() {
        crate::notify::error("The History window could not be opened.", e);
    }
}

/// Re-read the history into the open window (after a capture was recorded), keeping the
/// selection on the same file. Does nothing when the window is closed.
pub fn refresh() {
    if let Some(hwnd) = open_hwnd() {
        let keep = selected(hwnd).map(|e| e.path);
        reload(hwnd, keep.as_deref(), 0);
    }
}

fn try_open() -> Result<()> {
    ensure_class()?;
    let (work, dpi) = crate::platform::dpi::cursor_monitor();
    let px = |v: i32| crate::platform::dpi::scale(v, dpi);
    let ctx = Box::new(Ctx {
        entries: Vec::new(),
        preview: None,
        info: String::new(),
        dpi,
        font: crate::platform::dpi::UiFont::new(dpi),
    });
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    // SAFETY: window and child creation; the window adopts `ctx` in WM_NCCREATE and frees it in
    // WM_NCDESTROY.
    let hwnd = unsafe {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: px(CLIENT_W),
            bottom: px(CLIENT_H),
        };
        let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE(0), dpi);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let (sw, sh) = (work.right - work.left, work.bottom - work.top);
        let hinstance = GetModuleHandleW(None).context("GetModuleHandleW")?;
        PENDING.with(|p| *p.borrow_mut() = Some(ctx));
        let created = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("RustShot — History"),
            style,
            work.left + ((sw - ww) / 2).max(0),
            work.top + ((sh - wh) / 2).max(0),
            ww,
            wh,
            None,
            Some(HMENU(null_mut())),
            Some(hinstance.into()),
            None,
        );
        drop(PENDING.with(|p| p.borrow_mut().take()));
        created.context("creating the History window")?
    };
    OPEN.with(|o| o.set(hwnd.0 as isize));
    if let Err(e) = create_children(hwnd, dpi) {
        // SAFETY: our own window, never shown; WM_NCDESTROY frees its state.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
        }
        return Err(e);
    }
    reload(hwnd, None, 0);
    // SAFETY: show our own window.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    Ok(())
}

fn ensure_class() -> Result<()> {
    let mut err: Option<windows::core::Error> = None;
    REGISTER.call_once(|| {
        // SAFETY: one-time class registration.
        unsafe {
            let hinstance = match GetModuleHandleW(None) {
                Ok(h) => h,
                Err(e) => {
                    err = Some(e);
                    return;
                }
            };
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance.into(),
                lpszClassName: CLASS_NAME,
                hCursor: windows::Win32::UI::WindowsAndMessaging::LoadCursorW(
                    None,
                    windows::Win32::UI::WindowsAndMessaging::IDC_ARROW,
                )
                .unwrap_or_default(),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                err = Some(windows::core::Error::from_thread());
            }
        }
    });
    match err {
        Some(e) => Err(e).context("registering the History window class"),
        None => Ok(()),
    }
}

fn create_children(parent: HWND, dpi: u32) -> Result<()> {
    let px = |v: i32| crate::platform::dpi::scale(v, dpi);
    let font = ctx_of(parent).and_then(|c| c.font.as_ref().map(|f| f.0));
    // SAFETY: standard LISTBOX / BUTTON child creation on our own window.
    unsafe {
        let hinstance = GetModuleHandleW(None).context("GetModuleHandleW")?;
        let child = |class: PCWSTR, text: PCWSTR, style: u32, id: i32, r: (i32, i32, i32, i32)| {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                text,
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                px(r.0),
                px(r.1),
                px(r.2),
                px(r.3),
                Some(parent),
                Some(HMENU(id as isize as *mut c_void)),
                Some(hinstance.into()),
                None,
            )
        };
        let list_style =
            WS_VSCROLL.0 | WS_BORDER.0 | LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32;
        let mut made = vec![
            child(w!("LISTBOX"), PCWSTR::null(), list_style, ID_LIST, LIST)
                .context("creating the history list")?,
        ];
        let mut x = LIST.0;
        for (id, label, width) in BUTTONS {
            made.push(
                child(w!("BUTTON"), label, 0, id, (x, BUTTON_Y, width, BUTTON_H))
                    .context("creating a History button")?,
            );
            x += width + BUTTON_GAP;
        }
        if let Some(f) = font {
            for c in made {
                SendMessageW(c, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(1)));
            }
        }
    }
    Ok(())
}

fn ctx_of<'a>(hwnd: HWND) -> Option<&'a mut Ctx> {
    // SAFETY: userdata is null or the Box<Ctx> installed in WM_NCCREATE and freed in
    // WM_NCDESTROY.
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Ctx;
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}

/// Re-read the history (dropping entries whose file is gone), refill the list and select the
/// entry for `keep`, else the one at `index` (clamped), and update the preview.
fn reload(hwnd: HWND, keep: Option<&Path>, index: usize) {
    let entries = crate::history::load_existing().entries;
    let pick = keep
        .and_then(|k| entries.iter().position(|e| e.path == k))
        .unwrap_or_else(|| index.min(entries.len().saturating_sub(1)));
    let labels: Vec<HSTRING> = entries.iter().map(|e| HSTRING::from(e.label())).collect();
    let empty = entries.is_empty();
    if let Some(ctx) = ctx_of(hwnd) {
        ctx.entries = entries;
    }
    // SAFETY: list-box messages to our own child; the strings outlive each call.
    unsafe {
        if let Ok(list) = GetDlgItem(Some(hwnd), ID_LIST) {
            SendMessageW(list, LB_RESETCONTENT, None, None);
            for label in &labels {
                SendMessageW(
                    list,
                    LB_ADDSTRING,
                    None,
                    Some(LPARAM(label.as_ptr() as isize)),
                );
            }
            if !empty {
                SendMessageW(list, LB_SETCURSEL, Some(WPARAM(pick)), None);
            }
        }
    }
    on_select(hwnd);
}

/// The selected entry, copied out.
fn selected(hwnd: HWND) -> Option<Entry> {
    // SAFETY: a list-box query on our own child.
    let i = unsafe {
        let list = GetDlgItem(Some(hwnd), ID_LIST).ok()?;
        SendMessageW(list, LB_GETCURSEL, None, None).0
    };
    let i = usize::try_from(i).ok()?;
    ctx_of(hwnd)?.entries.get(i).cloned()
}

/// Show the selected entry: its preview, path and size, and which buttons apply.
fn on_select(hwnd: HWND) {
    let entry = selected(hwnd);
    let dpi = ctx_of(hwnd).map_or(crate::platform::dpi::BASE, |c| c.dpi);
    let px = |v: i32| crate::platform::dpi::scale(v, dpi);
    let (preview, info) = match &entry {
        None => (None, "No captures yet.".to_string()),
        Some(e) if e.kind.is_image() => match imaging::load_rgba(&e.path) {
            Ok(img) => {
                let size = format!("{} × {} px", img.width(), img.height());
                let thumb = imaging::thumbnail(&img, px(PREVIEW.2).max(px(PREVIEW.3)) as u32);
                let preview = Preview {
                    width: thumb.width() as i32,
                    height: thumb.height() as i32,
                    bgra: thumb
                        .as_raw()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|p| [p[2], p[1], p[0], 255])
                        .collect(),
                };
                (Some(preview), format!("{}\n{size}", e.path.display()))
            }
            Err(err) => (None, format!("{}\n\nNo preview: {err}", e.path.display())),
        },
        Some(e) => (
            None,
            format!(
                "{}\n\n{} — Open plays it in your default player.",
                e.path.display(),
                e.kind.label()
            ),
        ),
    };
    if let Some(ctx) = ctx_of(hwnd) {
        ctx.preview = preview;
        ctx.info = info;
    }
    let any = entry.is_some();
    let image = entry.as_ref().is_some_and(|e| e.kind.is_image());
    let has_entries = ctx_of(hwnd).is_some_and(|c| !c.entries.is_empty());
    let enabled = [
        (ID_OPEN, any),
        (ID_ANNOTATE, image),
        (ID_PIN, image),
        (ID_COPY, image),
        (ID_FOLDER, any),
        (ID_REMOVE, any),
        (ID_DELETE, any),
        (ID_CLEAR, has_entries),
    ];
    // SAFETY: enables our own child buttons and repaints our window.
    unsafe {
        for (id, on) in enabled {
            if let Ok(b) = GetDlgItem(Some(hwnd), id) {
                let _ = EnableWindow(b, on);
            }
        }
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    crate::panic_guard::guard(
        "history window wndproc",
        || unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        || unsafe { wndproc_inner(hwnd, msg, wparam, lparam) },
    )
}

unsafe fn wndproc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        if let Some(ctx) = PENDING.with(|p| p.borrow_mut().take()) {
            // SAFETY: store the owned pointer in our own window's userdata.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(ctx) as isize) };
        }
        // SAFETY: default processing of WM_NCCREATE.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_NCDESTROY {
        OPEN.with(|o| {
            if o.get() == hwnd.0 as isize {
                o.set(0);
            }
        });
        // SAFETY: last message: take back ownership and free the state exactly once.
        unsafe {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Ctx;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if id == ID_LIST {
                match code {
                    LBN_SELCHANGE => on_select(hwnd),
                    LBN_DBLCLK => command(hwnd, ID_OPEN),
                    _ => {}
                }
            } else {
                command(hwnd, id);
            }
            LRESULT(0)
        }
        // Keyboard focus to the list, so the arrow keys move through it.
        WM_ACTIVATE if (wparam.0 & 0xFFFF) != 0 => {
            // SAFETY: focuses our own child.
            unsafe {
                if let Ok(list) = GetDlgItem(Some(hwnd), ID_LIST) {
                    let _ = SetFocus(Some(list));
                }
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Carry out a button. The entry is copied out first; nothing holds the window state while
/// another window opens or a message box is up.
fn command(hwnd: HWND, id: i32) {
    if id == ID_CLEAR {
        clear(hwnd);
        return;
    }
    let Some(entry) = selected(hwnd) else {
        return;
    };
    let path = entry.path;
    match id {
        ID_OPEN => crate::ui::flyout::shell_open(&path),
        ID_ANNOTATE => crate::ui::annotate::open(&path),
        ID_PIN => crate::ui::pin::open(&path),
        ID_COPY => copy(&path),
        ID_FOLDER => crate::ui::flyout::select_in_explorer(&path),
        ID_REMOVE => {
            let index = current_index(hwnd);
            crate::history::update(|h| {
                h.remove(&path);
            });
            reload(hwnd, None, index);
        }
        ID_DELETE => delete_file(hwnd, &path),
        _ => {}
    }
}

fn current_index(hwnd: HWND) -> usize {
    // SAFETY: a list-box query on our own child.
    let i = unsafe {
        GetDlgItem(Some(hwnd), ID_LIST)
            .map(|list| SendMessageW(list, LB_GETCURSEL, None, None).0)
            .unwrap_or(0)
    };
    usize::try_from(i).unwrap_or(0)
}

fn copy(path: &Path) {
    let result = imaging::load_rgba(path)
        .map_err(anyhow::Error::from)
        .and_then(|img| crate::clipboard::copy_image(&img));
    if let Err(e) = result {
        crate::notify::warn("The image could not be copied to the clipboard.", e);
    }
}

/// Ask, then delete the file from disk and forget it.
fn delete_file(hwnd: HWND, path: &Path) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let question = format!(
        "Delete {name} from disk?\n\n{}\n\nThe file is deleted for good: it does not go to the Recycle Bin.",
        path.display()
    );
    if !confirm(hwnd, &question) {
        return;
    }
    let index = current_index(hwnd);
    match std::fs::remove_file(path) {
        Ok(()) => tracing::info!("deleted {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            crate::notify::warn(&format!("Could not delete {}", path.display()), e);
            return;
        }
    }
    crate::history::update(|h| {
        h.remove(path);
    });
    reload(hwnd, None, index);
}

/// Ask, then forget every entry (the files stay where they are).
fn clear(hwnd: HWND) {
    let n = ctx_of(hwnd).map_or(0, |c| c.entries.len());
    if n == 0 {
        return;
    }
    let question = format!(
        "Remove all {n} entries from the history?\n\nThe files themselves are not deleted."
    );
    if !confirm(hwnd, &question) {
        return;
    }
    crate::history::update(|h| h.entries.clear());
    reload(hwnd, None, 0);
}

/// A Yes/No question owned by the History window. Pumps messages: callers hold no state.
fn confirm(hwnd: HWND, question: &str) -> bool {
    let text = HSTRING::from(question);
    // SAFETY: a standard modal message box owned by our window.
    let answer = unsafe {
        MessageBoxW(
            Some(hwnd),
            PCWSTR(text.as_ptr()),
            w!("RustShot — History"),
            MB_YESNO | MB_ICONWARNING,
        )
    };
    answer == IDYES
}

fn paint(hwnd: HWND) {
    let Some(ctx) = ctx_of(hwnd) else {
        return;
    };
    let px = |v: i32| crate::platform::dpi::scale(v, ctx.dpi);
    let rect = |r: (i32, i32, i32, i32)| RECT {
        left: px(r.0),
        top: px(r.1),
        right: px(r.0 + r.2),
        bottom: px(r.1 + r.3),
    };
    // SAFETY: standard paint of our own window: background, preview box, thumbnail, info text.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let all = RECT {
            left: 0,
            top: 0,
            right: px(CLIENT_W),
            bottom: px(CLIENT_H),
        };
        FillRect(hdc, &all, GetSysColorBrush(COLOR_BTNFACE));

        let area = rect(PREVIEW);
        let dark = CreateSolidBrush(COLORREF(0x0020_2020));
        FillRect(hdc, &area, dark);
        let _ = DeleteObject(HGDIOBJ(dark.0));
        if let Some(p) = &ctx.preview {
            let (aw, ah) = (area.right - area.left, area.bottom - area.top);
            let scale = (f64::from(aw) / f64::from(p.width.max(1)))
                .min(f64::from(ah) / f64::from(p.height.max(1)))
                .min(1.0);
            let (dw, dh) = (
                (f64::from(p.width) * scale).round() as i32,
                (f64::from(p.height) * scale).round() as i32,
            );
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = p.width;
            bmi.bmiHeader.biHeight = -p.height; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            StretchDIBits(
                hdc,
                area.left + (aw - dw) / 2,
                area.top + (ah - dh) / 2,
                dw,
                dh,
                0,
                0,
                p.width,
                p.height,
                Some(p.bgra.as_ptr() as *const c_void),
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }

        let mut info_rect = rect(INFO);
        let mut text: Vec<u16> = ctx.info.encode_utf16().collect();
        let old = ctx
            .font
            .as_ref()
            .map(|f| SelectObject(hdc, HGDIOBJ(f.0 .0)));
        SetBkMode(hdc, TRANSPARENT);
        DrawTextW(
            hdc,
            &mut text,
            &mut info_rect,
            DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX,
        );
        if let Some(old) = old {
            SelectObject(hdc, old);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn buttons_fit_under_the_list_and_inside_the_window() {
        let total: i32 =
            BUTTONS.iter().map(|b| b.2).sum::<i32>() + BUTTON_GAP * (BUTTONS.len() as i32 - 1);
        assert!(
            LIST.0 + total <= CLIENT_W - 8,
            "buttons are {total} px wide"
        );
        // The fixed boxes do not overlap (checked when the tests compile).
        const {
            assert!(BUTTON_Y >= LIST.1 + LIST.3 + 4, "buttons below the list");
            assert!(BUTTON_Y + BUTTON_H <= CLIENT_H, "buttons inside the window");
            assert!(PREVIEW.0 >= LIST.0 + LIST.2 + 4 && PREVIEW.0 + PREVIEW.2 <= CLIENT_W);
            assert!(INFO.1 >= PREVIEW.1 + PREVIEW.3 && INFO.1 + INFO.3 <= BUTTON_Y);
        }
    }

    #[test]
    fn button_ids_are_distinct() {
        let mut ids: Vec<i32> = BUTTONS.iter().map(|b| b.0).chain([ID_LIST]).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BUTTONS.len() + 1);
    }
}
